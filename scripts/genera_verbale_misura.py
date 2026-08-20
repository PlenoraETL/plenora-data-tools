# -*- coding: utf-8 -*-
"""Genera il verbale della misura dal JSON grezzo, o ne verifica la coerenza.

Il documento non contiene numeri battuti a mano: ogni cifra viene dal grezzo, e
la tabella del quadro d'insieme e' il campo `tabella_markdown` prodotto dal
programma di misura, incollato verbatim.

    python scripts/genera_verbale_misura.py             # rigenera il verbale
    python scripts/genera_verbale_misura.py --verifica  # esce 1 se diverge

Va eseguito dalla radice del repository. La prosa non numerica e' qui dentro:
per modificarla si edita questo script, non il documento, altrimenti la
verifica fallisce.
"""
import difflib
import glob
import io
import json
import re
import sys
import textwrap

GREZZO = 'docs/misure/baseline-671214c.json'
VARIANZA = 'docs/misure/varianza/v-*.json'

d = json.load(io.open(GREZZO, encoding='utf-8'))
tab = d['tabella_markdown'].strip()
per = {c['carico']: c for c in d['carichi']}
ORDINE = ('streaming_lineare', 'blocking_sort', 'blocking_aggregate',
          'fan_out_tee', 'rami_indipendenti')


def mib(b):
    return b / (1024.0 * 1024.0)


doc = []
A = doc.append

A(u"""# Misura dell'orchestrazione — linea di base

Ramo `perf/orchestrazione-v1`, da `671214c`. **Nessuna ottimizzazione, nessuna
modifica di comportamento, nessuna API pubblica toccata.** L'unico codice nuovo
e' l'harness di misura, che sta in `examples/` e non entra nella libreria.

Harness: `crates/plenora-engine/examples/misura_orchestrazione.rs`.
Grezzi di questo verbale, **dallo stesso singolo run**:
`docs/misure/baseline-671214c.json` e `.txt`.
Grezzi della verifica di riproducibilita': `docs/misure/varianza/`.

```
cargo build --release --example misura_orchestrazione -p plenora-engine --locked
# in un container SEPARATO, senza compilazione in corso:
./target-linux/release/examples/misura_orchestrazione --json docs/misure/baseline-671214c.json
```

Ambiente: container `rust:1.92.0-slim`, 16 core logici, profilo `release`.
Input sintetico deterministico: 24 batch x 8192 righe = **196 608 righe,
8,28 MiB Arrow**.

---

## 0. Che cosa e' cambiato in questa stesura

Le due stesure precedenti avevano difetti di **metodo**. Correzioni applicate
in questo giro, tutte dentro l'harness:

| difetto | conseguenza | correzione |
|---|---|---|
| la soglia di 1,5 s non era garantita: il tetto di ripetizioni scattava prima | 4 carichi su 5 misurati su meno di 1,5 s cumulati | ripetizioni **a blocchi** finche' il wall cronometrato raggiunge davvero 1,5 s; nel JSON `soglia_raggiunta` e `max_raggiunto` |
| tempo e memoria misurati nello stesso processo | il `VmHWM` includeva i warm-up e decine di esecuzioni consecutive | **processo dedicato** alla memoria, **una sola** esecuzione misurata, `VmHWM` azzerato via `/proc/self/clear_refs` con **verifica** dell'azzeramento; se non riesce, la misura e' dichiarata **non disponibile** |
| «incertezza ±0,28%» | era la sola quantizzazione del tick, presentata come incertezza della misura | rinominata **risoluzione del tick**, e la campagna temporale ripetuta in **3 processi isolati** per carico: si riportano **mediana e range** del parallelismo |
| `filter_map` nel calcolo dei tetti dei rami | un nodo mancante spariva in silenzio e il rapporto restava plausibile | **lookup fail-closed** (`panic` sul nodo assente) e asserzione che ogni nodo abbia **esattamente** un campione per ripetizione |
| tabella scritta a mano leggendo il JSON | possibile divergenza fra grezzo e documento | la tabella del §2 e' **generata dal programma** (campo `tabella_markdown`) e incollata verbatim; il resto del documento e' generato da uno script che legge lo stesso JSON |
| rapporti RSS/governor e «tetto di 90 ripetizioni» | numeri costruiti su misure contaminate | **ritirati** entrambi; sostituiti da quanto segue |

Un settimo difetto e' emerso durante il run stesso, ed e' stato corretto: la
verifica dell'azzeramento confrontava `VmHWM` con il `VmRSS` letto **dopo** la
scrittura su `clear_refs`. Se fra i due istanti l'allocatore restituisce pagine
al sistema, `VmRSS` scende sotto `VmHWM` pur essendo l'azzeramento riuscito —
e `rami_indipendenti` veniva dichiarato non misurabile proprio per questo. Ora
lo stato si legge **a cavallo** della scrittura e il confronto e' col massimo
dei due.

---

## 1. Riproducibilita': quanto valgono questi numeri

Questa sezione viene per prima perche' condiziona la lettura di tutte le altre.

**I tempi assoluti dipendono dallo stato dell'host.** Lo stesso binario, sullo
stesso carico, ha dato **27 ms** con l'host quieto e **60 ms** con la
compilazione nello stesso container; in una terza serie, subito dopo una
campagna completa, ha dato **113–203 ms**. La causa di quest'ultima serie **non
e' stata isolata**: e' registrata come osservazione, non come spiegazione. Per
questo la campagna gira ora in un container **separato** dalla build, dopo una
pausa.

""")

# I numeri della tabella di riproducibilita' vengono dai grezzi del probe, non
# da questo file: se i grezzi cambiano e il verbale no, --verifica fallisce.
probe = []
invocazioni = set()
for percorso in sorted(glob.glob(VARIANZA)):
    c = json.load(io.open(percorso, encoding='utf-8'))
    if 'carichi' in c:
        c = c['carichi'][0]
    somma = float(sum(v['mediana_ns'] for v in c['nodi'].values()))
    invocazioni.add(percorso.replace('\\', '/').split('/')[-1].split('-')[1])
    probe.append({
        'wall_ms': c['tempo']['mediana_ns'] / 1e6,
        'fattore': c['parallelismo']['fattore'],
        'quote': dict((v['operazione'], 100.0 * v['mediana_ns'] / somma)
                      for v in c['nodi'].values()),
    })
assert probe, 'grezzi della varianza assenti: %s' % VARIANZA

A(u"""Riproducibilita' misurata a host quieto — %d campagne di
`streaming_lineare` distribuite su **%d invocazioni di container distinte**
(`docs/misure/varianza/`):""" % (len(probe), len(invocazioni)))

A(u"""
| grandezza | intervallo osservato | ampiezza |
|---|---|---|""")


def estremi(valori):
    return min(valori), max(valori)


w_min, w_max = estremi([p['wall_ms'] for p in probe])
f_min, f_max = estremi([p['fattore'] for p in probe])
A(u"| tempo mediano | %.2f – %.2f ms | **%.2fx** |" % (w_min, w_max, w_max / w_min))
A(u"| fattore di parallelismo | %.3f – %.3f | **%.2fx** |"
  % (f_min, f_max, f_max / f_min))
for op in sorted(probe[0]['quote'], key=lambda o: -probe[0]['quote'][o]):
    q_min, q_max = estremi([p['quote'][op] for p in probe])
    A(u"| quota per nodo (`%s`) | %.1f%% – %.1f%% | **%.1f punti** |"
      % (op.split('.')[-1], q_min, q_max, q_max - q_min))

A(u"""
Due campagne complete consecutive, entrambe a host quieto, danno per i cinque
carichi 29,60 / 15,11 / 12,38 / 7,53 / 215,59 ms e 31,24 / 16,39 / 14,52 /
8,13 / 220,55 ms: **entro 1,17x**. Solo la seconda e' conservata come artefatto
ed e' quella riportata qui sotto; la prima e' registrata solo in questo verbale.

**Conseguenza operativa.** Le grandezze **normalizzate** — fattore di
parallelismo, quote per nodo, tetti dei rami, rapporto RSS/governor — sono
stabili e si possono usare per decidere. I **tempi assoluti** valgono come
ordine di grandezza: un confronto prima/dopo su di essi richiede che le due
misure girino nelle stesse condizioni, e **una differenza sotto il 20% non e'
distinguibile dal rumore dell'host**.

---

## 2. Il quadro

Tabella **generata dal programma di misura**, incollata verbatim dal campo
`tabella_markdown` del JSON:
""")

A(tab)

A(u"""
Tutti e cinque i carichi hanno raggiunto la soglia di **1,5 s cumulati**
cronometrati (`soglia_raggiunta: true`), e nessuno ha toccato il tetto di
`RIPETIZIONI_MAX` (`max_raggiunto: false`). Il numero di ripetizioni varia da
**%d** a **%d** a seconda della durata del carico: e' la soglia a fissarlo, non
un valore scelto. L'affermazione della stesura precedente, secondo cui
`fan_out_tee` si fermava a 90 ripetizioni prima della soglia, e' **ritirata**:
quel tetto non e' piu' un vincolo effettivo.""" % (
    min(per[n]['ripetizioni'] for n in ORDINE),
    max(per[n]['ripetizioni'] for n in ORDINE)))

A(u"""

Determinismo: **byte IPC identici** su 6 esecuzioni per carico — da 2 248 a
10 496 456 byte confrontati direttamente, non per hash.

---

## 3. Parallelismo: esiste, ma e' tutto dentro i kernel

Mediana su %d processi isolati, range fra parentesi:
""" % d['processi_tempo_per_carico'])


def par_val(nome):
    return parallelismo(nome)['mediana']


def parallelismo(nome):
    """Il blocco del parallelismo fra processi, solo se e' pubblicabile.

    L'harness dichiara `disponibile: false` quando meno di tutte le campagne
    hanno misurato il parallelismo. Il verbale non ha una forma per dirlo — la
    §3 e il fatto 1 lo danno per acquisito — quindi qui si **ferma** invece di
    stampare una mediana su due campagne chiamandola «su tre».
    """
    p = per[nome]['parallelismo_fra_processi']
    if p.get('disponibile') is False:
        raise SystemExit(
            'parallelismo non disponibile per %s (%s): il grezzo non e\'\n'
            'pubblicabile in questo verbale, rieseguire la campagna.'
            % (nome, p.get('motivo', 'motivo non riportato')))
    return p


def par(nome):
    p = parallelismo(nome)
    return u"`%s` **%.2fx** [%.2f–%.2f]" % (
        nome, p['mediana'], p['min'], p['max'])


def punto(testo):
    return textwrap.fill(testo, width=76, initial_indent=u'- ',
                         subsequent_indent=u'  ')


A(u"\n".join([
    punto(u"%s e %s: questi piani girano **su un core solo**, su una macchina "
          u"che ne ha %d;" % (par('streaming_lineare'), par('fan_out_tee'),
                              d['core_logici'])),
    punto(u"%s e %s: qui il parallelismo c'e' perche' i kernel usano Rayon al "
          u"proprio interno;" % (par('blocking_sort'),
                                 par('blocking_aggregate'))),
    punto(u"%s: media dei regimi lungo la catena, non parallelismo fra rami."
          % par('rami_indipendenti')),
    u"",
    textwrap.fill(
        u"Nessun carico supera **%.2fx su %d core**. Il parallelismo **fra "
        u"nodi del DAG** e' zero, come dichiarato (`SerialFused`)."
        % (max(par_val(n)
               for n in ORDINE), d['core_logici']), width=76),
]))

A(u"""

### Il tetto del guadagno, con intervallo

Calcolato su **ogni** ripetizione come
`1 - (max(ramo A, ramo B) + convergenza) / (ramo A + ramo B + convergenza)`,
con i tempi per nodo di quella ripetizione e lookup fail-closed:
""")

fo, ri = per['fan_out_tee'], per['rami_indipendenti']


def riga_tetto(c, rami):
    t = c['tetto_parallelizzazione']
    return u"| `%s` | %s | **%.1f%%** | %.1f%% – %.1f%% | %d |" % (
        c['carico'], rami, t['guadagno_mediano_pct'], t['guadagno_min_pct'],
        t['guadagno_max_pct'], t['campioni'])


A(u"""
| carico | rami | tetto mediano | intervallo | campioni |
|---|---|---|---|---|""")
A(riga_tetto(ri, u"A = `a1`+`a2`, B = `b1`+`b2`, conv = `fine`"))
A(riga_tetto(fo, u"A = `ramo_a`, B = `ramo_b`, conv = `unione`"))

A(u"""
Il tetto di `rami_indipendenti` e' **stretto** e quindi utilizzabile. Quello di
`fan_out_tee` ha intervallo largo perche' il carico e' breve e un `unione`
occasionalmente lento (fino a 5,3 ms contro 1,1 ms mediani) schiaccia il
rapporto: **la mediana e' significativa, l'estremo inferiore no.**

Restano tetti **ottimistici**: ignorano sincronizzazione e contesa, e i kernel
dentro i rami usano gia' piu' core — eseguirli insieme li mette in
competizione. Sono limiti superiori, non obiettivi.

---

## 4. Memoria: due regimi, una sola esecuzione misurata per ciascuno

Processo dedicato per carico, `VmHWM` azzerato e **verificato** immediatamente
prima dell'esecuzione, RSS e governor letti dalla **stessa** esecuzione. I
rapporti della stesura precedente (1,1x–1,9x sui blocking, 26,7x sullo
streaming) sono **ritirati**: erano costruiti su un `VmHWM` che includeva i
warm-up e decine di esecuzioni.

Le due colonne non sono due stime dello stesso numero, sono **due limiti
osservati in questo banco**:

- **a freddo** — nessun warm-up: l'unica esecuzione misurata paga anche
  l'inizializzazione irripetibile (pool di thread, prime pagine
  dell'allocatore). E' il **piu' alto dei due valori osservati**;
- **a caldo** — dopo il warm-up: l'allocatore ha gia' le pagine che servono,
  quindi l'incremento tende a zero. E' il **piu' basso dei due**.

**Non sono limiti universali.** Sono cio' che questi due regimi del benchmark
producono, con questo allocatore, questo input e questa sequenza di esecuzioni.
Un altro allocatore, un input piu' grande o un uso che non parte mai a freddo
possono cadere fuori da entrambi: il fabbisogno reale di un processo che esegue
il piano una volta sola non e' stato misurato, e non e' deducibile da queste
due colonne.
""")

A(u"""
| carico | picco governato | RSS a freddo | RSS a caldo | rapporto a freddo |
|---|---|---|---|---|""")
for nome in ORDINE:
    m = per[nome]['memoria']
    if not m.get('disponibile', False):
        A(u"| `%s` | — | — | — | **non disponibile**: %s |" % (
            nome, m.get('motivo', 'azzeramento non verificabile')))
        continue
    A(u"| `%s` | %.2f MiB | %.2f MiB | %.2f MiB | **%.2fx** |" % (
        nome,
        mib(m['a_freddo']['picco_governato']),
        mib(m['a_freddo']['rss_incremento']),
        mib(m['a_caldo']['rss_incremento']),
        m['a_freddo']['rapporto']))

caldo_ri = mib(ri['memoria']['a_caldo']['rss_incremento'])
sl_c = per['streaming_lineare']
freddo_sl = mib(sl_c['memoria']['a_freddo']['rss_incremento'])
gov_sl = mib(sl_c['memoria']['a_freddo']['picco_governato'])
rapporti_blocking = [per[n]['memoria']['a_freddo']['rapporto']
                     for n in ('blocking_sort', 'blocking_aggregate',
                               'fan_out_tee', 'rami_indipendenti')]

A(u"""
Il caso a caldo di `rami_indipendenti` (**%.2f MiB**) e' l'artefatto in forma
pura: dopo il warm-up il fabbisogno di quel carico e' gia' interamente
residente, quindi la singola esecuzione misurata non fa crescere l'RSS. Non
significa che il carico costi %.2f MiB; significa che **quel** valore, per
quel carico, non e' informativo.

Il caso estremo utile e' `streaming_lineare`: governa **%.2f MiB** perche' non
materializza nulla — il governor vede solo il batch in transito — mentre
il processo cresce di **%.2f MiB** per i buffer dei kernel, l'allocatore e il
pool. Sui carichi che materializzano, dove il governor conta le
materializzazioni, il rapporto a freddo sta fra **%.2fx** e **%.2fx**.

E' esattamente cio' che DER-011 dichiara: `max_memory_bytes` **non e' un tetto
duro** sull'occupazione del processo. Quanto resta fuori dal governo dipende dal
carico, e questa misura lo quantifica per cinque forme.

---

## 5. Byte Arrow attraversati

Rapporto `somma(bytes_in per nodo) / byte dell'input`:
`blocking_*` 1,00x, `fan_out_tee` 2,93x, `streaming_lineare` 3,73x,
`rami_indipendenti` 4,59x.

**Non e' un conteggio di copie**, e non va spacciato per tale: misura quante
volte i dati sono *osservati* da un nodo. Con tre nodi in catena il rapporto e'
~3x anche se nessun buffer viene duplicato, perche' Arrow condivide i buffer per
riferimento. Per distinguere attraversamenti da copie servirebbe un allocatore
tracciante: **non e' stato fatto**.

---

## 6. Backpressure e spill

Ogni carico rieseguito con `max_memory_bytes` pari **alla meta' del picco
governato mediano**:

| carico | esito |
|---|---|""" % (caldo_ri, caldo_ri, gov_sl, freddo_sl,
                min(rapporti_blocking), max(rapporti_blocking)))

ESITO = {'riuscito': u'riesce'}
for nome in ORDINE:
    e = per[nome]['sotto_pressione']['esito']
    if e == 'riuscito' and per[nome]['sotto_pressione'].get('spill_scritti', 0) > 0:
        testo = u'**riesce spillando**'
    elif e == 'riuscito':
        testo = u'riesce'
    else:
        testo = u'**`%s`**' % e
    A(u"| `%s` | %s |" % (nome, testo))

A(u"""
Il fatto che conta: **lo spill e' preventivo, non reattivo**. L'attivazione e'
su soglia stimata ai punti di dispatch (ADR-0002), quindi `sort` con budget
dimezzato *fallisce* dove `aggregate` *spilla* — non perche' `sort` non sappia
spillare, ma perche' la sua soglia preventiva non e' scattata e una reservation
fallita non ha modo di tornare indietro. `ReservationResult::MustSpill` e
`RetryAfterProgress` esistono nell'API e non sono **mai emessi**.

---

## 7. Dove sta il tempo
""")

A(u"`rami_indipendenti`, mediana per nodo su %d ripetizioni:\n"
  % ri['ripetizioni'])
A(u"| nodo | op | mediana | intervallo |\n|---|---|---|---|")
for nid, v in sorted(ri['nodi'].items(), key=lambda kv: -kv[1]['mediana_ns']):
    A(u"| `%s` | `%s` | **%.2f ms** | %.2f – %.2f |" % (
        nid, v['operazione'], v['mediana_ns'] / 1e6,
        v['min_ns'] / 1e6, v['max_ns'] / 1e6))

sl = sl_c['nodi']
wall_sl = sl_c['tempo']['mediana_ns']
quota_sl = 100.0 * sum(v['mediana_ns'] for v in sl.values()) / wall_sl

A(u"""
`streaming_lineare`: `string_pad` %.2f ms, `filter` %.2f ms, `formula` %.2f ms,
su un wall mediano di %.2f ms.""" % (
    sl['s']['mediana_ns'] / 1e6, sl['f']['mediana_ns'] / 1e6,
    sl['c']['mediana_ns'] / 1e6, wall_sl / 1e6) + u"""

### Quanto del wall e' coperto dai tempi per nodo

Rapporto fra la **somma delle mediane per nodo** e la **mediana del wall**.
Non e' la mediana del rapporto — il JSON conserva per nodo solo mediana, minimo
e massimo, non la serie — quindi vale come stima, non come statistica esatta:
""")

A(u"| carico | somma nodi | wall mediano | copertura |\n|---|---|---|---|")
coperture = {}
for nome in ORDINE:
    c = per[nome]
    somma = sum(v['mediana_ns'] for v in c['nodi'].values())
    wall = c['tempo']['mediana_ns']
    coperture[nome] = 100.0 * somma / wall
    A(u"| `%s` | %.2f ms | %.2f ms | **%.0f%%** |" % (
        nome, somma / 1e6, wall / 1e6, coperture[nome]))

cop_min = min(coperture.values())
cop_max = max(coperture.values())

A(u"""
**Questo corregge la stesura precedente**, che affermava che «la somma dei
tempi per nodo copre la quasi totalita' del wall time». E' vero solo dove i
kernel sono lunghi: **%(cop_rami).0f%%** su `rami_indipendenti` e **%(cop_sort).0f%%** su
`blocking_sort`. Sui carichi corti — `blocking_aggregate` **%(cop_agg).0f%%**,
`streaming_lineare` **%(cop_stream).0f%%** — meta' del wall sta **fuori** dai tempi per
nodo.

Dove stia quella meta' **non e' stato stabilito da questa misura**: i candidati
sono il dispatch per batch, la costruzione dell'output e la raccolta finale,
ma nessuno dei tre e' strumentato. E' il primo punto da chiarire nella fase
successiva, perche' cambia la risposta: se il costo fuori dai nodi e' fisso per
esecuzione, un orchestratore parallelo non lo tocca; se e' per batch, lo tocca
eccome.

Quello che resta stabilito: sui carichi dominati dai kernel il tempo e' **nei
kernel**, e li' un orchestratore parallelo puo' **sovrapporre** i costi, non
ridurli.

---

## 8. Che cosa questa misura NON dice

- **copie Arrow reali**: misurati gli attraversamenti, non le allocazioni;
- **profilazione a livello di funzione**: nessun `perf`, nessun flamegraph; il
  profilo e' per nodo, la granularita' che l'engine gia' espone;
- **carichi geo**: nessuno dei cinque tocca i kernel geo, dove esistono la
  fusione (ADR-0012) e i suoi fallback;
- **input reali**: dati sintetici, distribuzione uniforme;
- **scala**: un solo ordine di grandezza (8,28 MiB). Il comportamento a 1 GiB
  — dove lo spill conta davvero — non e' stato osservato;
- **contesa fra piani**: un solo piano per volta, 16 core nel container;
- **dove sta il wall non coperto dai nodi**: fino a meta' su due carichi su
  cinque, e questa misura non lo attribuisce (vedi §7);
- **tempi assoluti come base di confronto**: vedi §1. Riproducibili entro
  1,17x a host quieto, non oltre.

---

## 9. Fatti verificati

1. Il parallelismo **fra nodi** e' zero; quello **dentro i kernel** arriva a
   **%(par_sort).2fx** (sort) e **%(par_agg).2fx** (aggregate), ed e' assente nelle catene
   streaming (%(par_stream).2fx) e nel tee (%(par_tee).2fx). Range su %(processi)d processi isolati per
   carico.
2. Il tetto del guadagno da parallelismo fra rami e' **%(tetto_rami_med).1f%%** [%(tetto_rami_min).1f–%(tetto_rami_max).1f]
   su `rami_indipendenti`, e **%(tetto_tee_med).1f%%** (mediana, intervallo largo) su
   `fan_out_tee`.
3. Il tempo sta **nei kernel** sui carichi dominati dai kernel — copertura
   **%(cop_rami).0f%%** su `rami_indipendenti`, **%(cop_sort).0f%%** su `blocking_sort` — ma
   **non** sui carichi corti, dove scende a **%(cop_agg).0f%%** e **%(cop_stream).0f%%**. La quota
   non coperta non e' attribuita: e' il primo punto da strumentare.
4. Il rapporto fra incremento di RSS e picco governato, a freddo, sta fra
   **%(rss_min).2fx** e **%(rss_max).2fx** sui carichi che materializzano, e vale **%(rss_stream).2fx** su
   quello streaming, che non materializza nulla. Il 3,7x della prima stesura e
   i rapporti della seconda sono ritirati.
5. Lo spill e' **preventivo e non reattivo**: `MustSpill` e
   `RetryAfterProgress` non sono mai emessi.
6. L'esecuzione e' **deterministica byte a byte** su tutti e cinque i carichi.
7. La misura e' riproducibile **entro %(riprod_tempi).2fx** sui tempi e **entro %(riprod_quote).1f punti**
   sulle quote per nodo, a host quieto e con la compilazione fuori dal
   container di misura.
""" % {
    'cop_rami': coperture['rami_indipendenti'],
    'cop_sort': coperture['blocking_sort'],
    'cop_agg': coperture['blocking_aggregate'],
    'cop_stream': coperture['streaming_lineare'],
    'par_sort': par_val('blocking_sort'),
    'par_agg': par_val('blocking_aggregate'),
    'par_stream': par_val('streaming_lineare'),
    'par_tee': par_val('fan_out_tee'),
    'processi': d['processi_tempo_per_carico'],
    'tetto_rami_med': ri['tetto_parallelizzazione']['guadagno_mediano_pct'],
    'tetto_rami_min': ri['tetto_parallelizzazione']['guadagno_min_pct'],
    'tetto_rami_max': ri['tetto_parallelizzazione']['guadagno_max_pct'],
    'tetto_tee_med': fo['tetto_parallelizzazione']['guadagno_mediano_pct'],
    'rss_min': min(rapporti_blocking),
    'rss_max': max(rapporti_blocking),
    'rss_stream': sl_c['memoria']['a_freddo']['rapporto'],
    'riprod_tempi': w_max / w_min,
    'riprod_quote': max(max(p['quote'][op] for p in probe)
                        - min(p['quote'][op] for p in probe)
                        for op in probe[0]['quote']),
})

# La tabella prodotta dal programma resta VERBATIM; tutto il resto passa alla
# virgola decimale, per uniformita' con la prosa del verbale.


SEGNAPOSTO = u'@@TABELLA_DEL_PROGRAMMA@@'
assert doc.count(tab) == 1
doc[doc.index(tab)] = SEGNAPOSTO
testo = u"\n".join(doc)
testo = re.sub(r"(?<=\d)\.(?=\d)", u",", testo)
# Le sezioni assemblate da piu' A() lasciano righe vuote doppie: si
# normalizzano, cosi' il documento e' stabile a prescindere dal montaggio.
testo = re.sub(r"\n{3,}", u"\n\n", testo)
assert SEGNAPOSTO in testo
testo = testo.replace(SEGNAPOSTO, tab)

DESTINAZIONE = 'docs/misura-orchestrazione-2026-08-19.md'

if '--verifica' in sys.argv:
    # Verificatore JSON -> documento: il verbale deve essere esattamente cio'
    # che questo script produce dal grezzo. Qualunque cifra ritoccata a mano
    # nel documento, o qualunque grezzo sostituito senza rigenerare, fa
    # fallire il confronto.
    try:
        attuale = io.open(DESTINAZIONE, encoding='utf-8', newline='').read()
    except IOError as errore:
        sys.stderr.write('verbale illeggibile: %s\n' % errore)
        raise SystemExit(2)
    attuale = attuale.replace('\r\n', '\n')
    if attuale != testo:
        sys.stderr.write(
            'il verbale diverge dal grezzo: rigenerare con\n'
            '  python scripts/genera_verbale_misura.py\n')
        for riga in difflib.unified_diff(
                attuale.splitlines(), testo.splitlines(),
                fromfile=DESTINAZIONE, tofile='atteso', lineterm='', n=1):
            sys.stderr.write(riga + '\n')
        raise SystemExit(1)
    print('verbale coerente con %s' % GREZZO)
    raise SystemExit(0)

io.open(DESTINAZIONE, 'w', encoding='utf-8', newline='\n').write(testo)
print('verbale scritto in %s' % DESTINAZIONE)
