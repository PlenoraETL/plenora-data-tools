# -*- coding: utf-8 -*-
"""Genera il verbale del benchmark M2d (prima/dopo), o lo verifica.

    python scripts/genera_verbale_m2d.py             # rigenera
    python scripts/genera_verbale_m2d.py --verifica  # esce 1 se diverge

Va eseguito dalla radice del repository. Confronta gli stessi grezzi:
- PRIMA: docs/misure/decomposizione/ e docs/misure/baseline-671214c.json
- DOPO:  docs/misure/m2d-dopo/
"""
import difflib
import io
import json
import re
import sys

PRIMA_DEC = 'docs/misure/decomposizione'
DOPO = 'docs/misure/m2d-dopo'
PRIMA_BASE = 'docs/misure/baseline-671214c.json'
DESTINAZIONE = 'docs/m2d-staging-memory-first-2026-08-21.md'
ORDINE = ('streaming_lineare', 'blocking_sort', 'blocking_aggregate',
          'fan_out_tee', 'rami_indipendenti')
# I carichi che NON passano dal percorso row-diagnostics: controlli.
CONTROLLI = ('blocking_sort', 'blocking_aggregate', 'fan_out_tee',
             'rami_indipendenti')


def carica(percorso):
    return json.load(io.open(percorso, encoding='utf-8'))


dec_prima = dict((n, carica('%s/dec-%s.json' % (PRIMA_DEC, n))) for n in ORDINE)
dec_dopo = dict((n, carica('%s/dec-%s.json' % (DOPO, n))) for n in ORDINE)
cat_prima = carica('%s/catena.json' % PRIMA_DEC)
cat_dopo = carica('%s/catena.json' % DOPO)
base_prima = {c['carico']: c for c in carica(PRIMA_BASE)['carichi']}
base_dopo = {c['carico']: c for c in carica('%s/baseline.json' % DOPO)['carichi']}


def canonica(dec, nome):
    scelte = [c for c in dec[nome]['asse_batch']['forme'] if c['batch'] == 24]
    assert len(scelte) == 1, 'forma canonica assente: %s' % nome
    return scelte[0]


def op(cat, nome):
    scelte = [e for e in cat['per_operazione'] if e['operazione'] == nome]
    assert len(scelte) == 1, 'operazione assente: %s' % nome
    return scelte[0]['misura']


def guadagno_relativo(prima_soggetto, dopo_soggetto, prima_ctrl, dopo_ctrl):
    """Guadagno del soggetto NORMALIZZATO su un controllo della stessa
    campagna.

    I tempi assoluti di questo host variano fra campagne piu' del 20% (vedi
    `docs/misura-orchestrazione-2026-08-19.md` §1): un confronto diretto
    prima/dopo misurerebbe in parte lo stato dell'host. Il rapporto
    soggetto/controllo e' invece interno alla campagna, e il controllo non
    passa dal codice cambiato.
    """
    rapporto_prima = float(prima_soggetto) / float(prima_ctrl)
    rapporto_dopo = float(dopo_soggetto) / float(dopo_ctrl)
    return 100.0 * (rapporto_prima - rapporto_dopo) / rapporto_prima


sl_prima, sl_dopo = canonica(dec_prima, 'streaming_lineare'), canonica(dec_dopo, 'streaming_lineare')
f_prima, f_dopo = op(cat_prima, 'solo_formula'), op(cat_dopo, 'solo_formula')
sp_prima, sp_dopo = op(cat_prima, 'solo_string_pad'), op(cat_dopo, 'solo_string_pad')

g_formula = guadagno_relativo(f_prima['wall_ns'], f_dopo['wall_ns'],
                              sp_prima['wall_ns'], sp_dopo['wall_ns'])
g_streaming = guadagno_relativo(
    base_prima['streaming_lineare']['tempo']['mediana_ns'],
    base_dopo['streaming_lineare']['tempo']['mediana_ns'],
    base_prima['blocking_sort']['tempo']['mediana_ns'],
    base_dopo['blocking_sort']['tempo']['mediana_ns'])

doc = []
A = doc.append

A(u"""# M2d — staging memory-first: che cosa e' cambiato, misurato

Ramo `perf/orchestrazione-v1`. Una sola ottimizzazione, causale: gli accepted
dei segmenti row-diagnostics attendono **in memoria** invece che su file
temporaneo, con passaggio deterministico al disco quando il budget non basta.
La barriera R9.9 e' invariata; nessuna operazione e' specializzata;
`emits_row_diagnostics` non e' toccato. Vedi ADR-0002 §M2d.

Grezzi: PRIMA `docs/misure/decomposizione/` e `docs/misure/baseline-671214c.json`;
DOPO `docs/misure/m2d-dopo/`. Stessi carichi, stesso harness, stessa forma
dell'input.

---

## 1. Come si legge un confronto prima/dopo su questo host

I tempi assoluti variano fra campagne piu' del 20% per stato dell'host
(`docs/misura-orchestrazione-2026-08-19.md` §1). In questo benchmark i
carichi di controllo — quelli che **non** passano dal codice cambiato —
si spostano del 14–32% da soli: e' rumore, non effetto.

Le conclusioni si leggono quindi su due quantita' immuni allo stato
dell'host:

- il **residuo** e il **costo per riga**, gia' normalizzati (§2);
- il **rapporto soggetto/controllo** misurato dentro la stessa campagna (§3).

I tempi assoluti sono riportati, ma non sono l'argomento.

---

## 2. L'effetto, in quantita' normalizzate

Quota del wall attribuita ai kernel e residuo non attribuito, alla forma
canonica (24 batch x 8192 righe), e costo per riga del residuo dall'asse
delle righe:
""")

A(u"""
| carico | kernel % | residuo % | costo per riga |
|---|---|---|---|""")
for n in ORDINE:
    a, b = canonica(dec_prima, n), canonica(dec_dopo, n)
    ra = dec_prima[n]['asse_righe']['residuo']['per_riga_ns']
    rb = dec_dopo[n]['asse_righe']['residuo']['per_riga_ns']
    A(u"| `%s` | %.1f%% → **%.1f%%** | %.1f%% → **%.1f%%** | %.1f → **%.1f** ns |" % (
        n, a['quote_pct']['kernel'], b['quote_pct']['kernel'],
        a['quote_pct']['residuo'], b['quote_pct']['residuo'], ra, rb))

A(u"""
`streaming_lineare` — l'unico dei cinque con un nodo `formula` — passa da
**%.1f%%** a **%.1f%%** di residuo, e il costo per riga da **%.1f ns** a
**%.1f ns**. Sui quattro controlli la **quota di residuo** resta dov'era,
entro pochi punti. Il loro **costo per riga** si muove di piu' (fino a ~60%%
in questa campagna) pur non essendo toccato dal codice: e' la stessa
oscillazione dell'host che si vede nei tempi assoluti al §3, e va letta come
tale — non come un effetto della modifica, che su quei carichi non esegue una
riga diversa da prima.

Sui piani a nodo singolo l'effetto e' ancora piu' netto, perche' non c'e'
altro nel wall:
""" % (sl_prima['quote_pct']['residuo'], sl_dopo['quote_pct']['residuo'],
       dec_prima['streaming_lineare']['asse_righe']['residuo']['per_riga_ns'],
       dec_dopo['streaming_lineare']['asse_righe']['residuo']['per_riga_ns']))

A(u"| piano | wall | kernel | residuo | residuo % |")
A(u"|---|---|---|---|---|")
for nome in ('solo_formula', 'solo_formula_costante', 'solo_formula_intero',
             'solo_string_pad', 'solo_filter'):
    a, b = op(cat_prima, nome), op(cat_dopo, nome)
    A(u"| `%s` | %.2f → **%.2f** ms | %.2f → %.2f ms | %.2f → **%.2f** ms | %.1f%% → **%.1f%%** |" % (
        nome, a['wall_ns'] / 1e6, b['wall_ns'] / 1e6,
        a['partizione_ns']['kernel'] / 1e6, b['partizione_ns']['kernel'] / 1e6,
        a['partizione_ns']['residuo'] / 1e6, b['partizione_ns']['residuo'] / 1e6,
        a['quote_pct']['residuo'], b['quote_pct']['residuo']))

A(u"""
Il residuo di `solo_formula` scende da **%.2f ms** a **%.2f ms**: lo staging
IPC e il suo replay, che valevano l'84–86%% di quel residuo, non vengono piu'
eseguiti. `string_pad` e `filter`, che non passavano da quel percorso, non
cambiano natura.

---

## 3. Il guadagno, normalizzato su un controllo interno alla campagna

Rapporto fra il soggetto e un carico di controllo misurato **nella stessa
campagna**: elimina lo stato dell'host, perche' entrambi lo subiscono.
""" % (f_prima['partizione_ns']['residuo'] / 1e6,
       f_dopo['partizione_ns']['residuo'] / 1e6))

A(u"""
| soggetto | controllo | rapporto prima | rapporto dopo | guadagno |
|---|---|---|---|---|""")
A(u"| `solo_formula` | `solo_string_pad` | %.3f | %.3f | **%.0f%%** |" % (
    f_prima['wall_ns'] / float(sp_prima['wall_ns']),
    f_dopo['wall_ns'] / float(sp_dopo['wall_ns']), g_formula))
A(u"| `streaming_lineare` | `blocking_sort` | %.3f | %.3f | **%.0f%%** |" % (
    base_prima['streaming_lineare']['tempo']['mediana_ns']
    / float(base_prima['blocking_sort']['tempo']['mediana_ns']),
    base_dopo['streaming_lineare']['tempo']['mediana_ns']
    / float(base_dopo['blocking_sort']['tempo']['mediana_ns']), g_streaming))

A(u"""
Entrambi superano largamente la soglia concordata del 15%.

Per completezza, i tempi assoluti — **da leggere come ordine di grandezza**,
non come misura del guadagno:
""")

A(u"| carico | wall prima | wall dopo | variazione del wall |")
A(u"|---|---|---|---|")
for n in ORDINE:
    wa = base_prima[n]['tempo']['mediana_ns'] / 1e6
    wb = base_dopo[n]['tempo']['mediana_ns'] / 1e6
    A(u"| `%s` | %.2f ms | %.2f ms | %+.1f%% |" % (n, wa, wb, -100.0 * (wa - wb) / wa))

controlli_residuo = [abs(canonica(dec_dopo, n)['quote_pct']['residuo']
                         - canonica(dec_prima, n)['quote_pct']['residuo'])
                     for n in CONTROLLI]

A(u"""
I quattro controlli si spostano fra il 14%% e il 29%% **in tempo assoluto** pur
non essendo toccati dal codice: e' la dimostrazione diretta che su questo host
il tempo assoluto non misura l'effetto. Le loro quote di residuo, che invece
sono normalizzate, si muovono al massimo di **%.1f punti**: nessuna
regressione oltre la soglia del 5%% concordata.

---

## 4. Prezzo: il picco governato

Trattenere gli accepted significa trattenere i loro lease. Il picco governato
cresce di conseguenza, ed e' l'unico costo di questa modifica:
""" % (max(controlli_residuo),))

A(u"| carico | picco governato prima | dopo | aumento |")
A(u"|---|---|---|---|")
for n in ORDINE:
    ga = base_prima[n]['picco_governato_mediano']
    gb = base_dopo[n]['picco_governato_mediano']
    aumento = u"**+%.2f MiB**" % ((gb - ga) / 1048576.0) if gb != ga else u"invariato"
    A(u"| `%s` | %.2f MiB | %.2f MiB | %s |" % (
        n, ga / 1048576.0, gb / 1048576.0, aumento))

sl_gov_dopo = base_dopo['streaming_lineare']['picco_governato_mediano'] / 1048576.0
budget = base_dopo['streaming_lineare']['memoria']['a_freddo'].get('picco_governato', 0)

A(u"""
L'aumento e' concentrato dove il percorso e' cambiato: `streaming_lineare`
passa da 0,76 MiB a **%.2f MiB**, cioe' i ventiquattro batch di output
trattenuti. Gli altri quattro non hanno nodi row-diagnostics e restano
identici al byte.

**%.2f MiB su un budget di 512 MiB e' il 2%%.** E non e' un tetto sfondato: la
soglia garantisce per costruzione che il picco resti sotto `max_memory_bytes`
(ADR-0002 §M2d), e il test `m2d_picco_governato_memoria_non_supera_il_budget`
lo verifica su entrambe le modalita'.

## 5. Frequenza del fallback

Il passaggio al disco e' **una tantum per esecuzione**: una volta avvenuto, la
scansione resta su disco. Non e' quindi una frequenza per batch ma un
booleano per esecuzione, e la sua condizione e' deterministica.

Sui cinque carichi, con i limiti predefiniti (`max_memory_bytes` 512 MiB,
`max_batch_bytes` 64 MiB) il fallback **non e' mai scattato**: il massimo
trattenuto e' %.2f MiB, e `%.2f + 0,42 + 64 = %.2f MiB` resta ampiamente sotto
il budget. Perche' scatti servirebbe un budget sotto i ~75 MiB.

Che il fallback funzioni, e che preservi il comportamento precedente, e'
verificato dai test invece che dedotto:

- `m2d_soglia_esatta_e_attraversamento` esercita **entrambi i lati** della
  soglia: con budget ampio la quota temporanea di 1 byte non viene nemmeno
  toccata (prova che non si scrive su disco); con budget stretto si passa a
  disco e quella stessa quota fa fallire;
- `m2d_budget_stretto_non_regredisce_a_resource_limit` esegue lo stesso piano
  con budget di 64 KiB, 256 KiB e 1 MiB — tutti sotto il tetto per batch,
  quindi in modalita' disco dal primo batch — e verifica che **riesca**, con
  il picco dentro il budget. E' il rischio dichiarato di questa modifica, ed
  e' chiuso da un test invece che da un argomento.

---

## 6. Equivalenza fra le due modalita'

| verifica | esito |
|---|---|
| byte IPC identici memoria vs disco | `m2d_memoria_e_disco_producono_gli_stessi_byte` |
| ordine di produzione | `m2d_ordine_e_sequenza_logica_preservati` |
| `BatchSequence` identica | `m2d_batch_sequence_identica_fra_modalita` |
| rejection tardiva: zero accepted | `m2d_rejection_tardiva_non_pubblica_nulla_in_memoria` |
| cancellazione: zero accepted | `m2d_cancellazione_dopo_accepted_trattenuti_non_pubblica_nulla` |
| soglia e attraversamento | `m2d_soglia_esatta_e_attraversamento` |
| quota temporanea fail-closed | `accepted_output_staging_beyond_temp_quota_fails_closed` |
| lease tutti rilasciati | `m2d_lease_rilasciati_a_fine_esecuzione` |
| picco dentro il budget | `m2d_picco_governato_memoria_non_supera_il_budget` |
| consumo da un segmento a valle | `m2d_output_consumato_da_un_segmento_successivo` |
| dictionary e nested | `m2d_dictionary_e_nested` |
| batch vuoti | `m2d_zero_colonne_e_batch_vuoti` |
| tre famiglie row-diagnostics | `m2d_tre_famiglie_row_diagnostics` |
| nessun falso `ResourceLimit` | `m2d_budget_stretto_non_regredisce_a_resource_limit` |

In piu', la campagna completa dell'harness verifica il determinismo byte a
byte su tutti e cinque i carichi: i byte IPC confrontati sono **identici a
quelli di prima della modifica**, con lo stesso conteggio esatto.

---

## 7. La soglia: la fonte era locale, ora e' globale

La prima stesura della soglia sommava i soli accepted trattenuti piu' il
batch d'ingresso corrente. Non e' tutto cio' che e' vivo nel governor: in un
fan-out `EdgeShared` conserva i batch gia' prelevati per il consumatore piu'
lento e ne trattiene i lease, che nessun contatore locale del ramo
row-diagnostics puo' vedere. La prova di sicurezza aveva quindi un buco: si
appoggiava a una somma parziale.

La soglia usa ora `governor.reserved_bytes()`, che e' la fonte unica —
accepted trattenuti, lease del batch corrente, buffer del tee e qualunque
altra prenotazione viva, presente o futura — piu' il solo headroom per
`max_batch_bytes`. Il contatore locale era un duplicato **parziale** ed e'
stato eliminato. Un ingresso senza lease non e' contabilizzato dal governor:
in quel caso si va su disco, fail-closed.

### Che cosa ho potuto dimostrare, e che cosa no

La regressione `m2d_fanout_nessun_falso_resource_limit` costruisce il fan-out
descritto — due rami row-diagnostics, il tee che trattiene mentre il primo
viene drenato, entrambi gli ordini dei rami, uno sweep di budget — e verifica
l'invariante: dove il percorso disco riesce, quello memory-first deve riuscire
con lo stesso risultato e senza sfondare il budget.

**Non sono riuscito a costruire un input in cui la soglia precedente
fallisse davvero**, e il test infatti passa anche con quella ripristinata. Ho
verificato perche', misurando invece di supporre: in un fan-out i due rami
devono riconvergere, e in v1 sempre attraverso un nodo che materializza
(`concat`/`join` binari, `BinaryBlocking`). Quel nodo drena e trattiene
comunque tutti i batch del ramo, quindi il **picco governato e' identico nelle
due modalita'** — misurato: 143 744 byte sia in memoria sia su disco. Dove il
tee trattiene, la memoria non aggiunge nulla al picco; dove la memoria alza il
picco (uscita diretta al piano, come `streaming_lineare`) non c'e' tee. Sul
motore attuale le due condizioni sembrano escludersi.

Questo **non rende la correzione superflua**: la sicurezza della soglia
precedente dipendeva da un accoppiamento accidentale — `max_batch_bytes`
finiva per essere maggiore delle prenotazioni non contate — che nessun
invariante garantisce e che un binario streaming, un nuovo punto di ritenzione
o lo scheduler parallelo romperebbero in silenzio. Il test resta come guardia:
e' la prima topologia in cui la differenza si vedrebbe.

### Vincolo per lo scheduler parallelo

Con l'esecuzione seriale, fra la lettura di `reserved_bytes()` e la
reservation della passata non si inserisce nessun altro: lo snapshot e'
stabile. Uno scheduler parallelo **non puo' conservare questa forma**: la
finestra fra `check` e `reserve` diventerebbe un TOCTOU. Serve un permesso
atomico del governor — una sola operazione che verifichi e riservi, o rifiuti.
Registrato in ADR-0002 §M2d come vincolo su M3.

---

## 8. Che cosa NON e' cambiato e che cosa resta aperto

- **`emits_row_diagnostics` e' invariato**: nessuna operazione e' stata tolta
  dalla lista ne' specializzata. Cambia dove gli accepted attendono, non chi
  passa dalla barriera;
- **il gate WKB dell'input resta su disco**: fuori dal perimetro di questo
  blocco. Il braccio corrispondente e' fail-closed, non silenzioso;
- **scheduler, API pubbliche e semantica delle metriche**: non toccati;
- **le altre operazioni row-diagnostics** (`expression`, `assert_*`, `date_*`,
  `flatten_json`) beneficiano dello stesso cambiamento perche' condividono il
  percorso, ma **non sono state misurate**: solo tre famiglie sono coperte dai
  test di equivalenza;
- **la materializzazione dei nodi bloccanti** (§5 del documento di
  decomposizione) e' un'altra causa, non toccata qui: i controlli mostrano
  infatti il loro residuo invariato.
""" % (sl_gov_dopo, sl_gov_dopo,
       sl_gov_dopo, sl_gov_dopo, sl_gov_dopo + 0.42 + 64.0))

testo = u"\n".join(doc)
testo = re.sub(r"(?<=\d)\.(?=\d)", u",", testo)
testo = re.sub(r"\n{3,}", u"\n\n", testo)

if '--verifica' in sys.argv:
    try:
        attuale = io.open(DESTINAZIONE, encoding='utf-8', newline='').read()
    except IOError as errore:
        sys.stderr.write('verbale illeggibile: %s\n' % errore)
        raise SystemExit(2)
    if attuale.replace('\r\n', '\n') != testo:
        sys.stderr.write('il verbale diverge dai grezzi: rigenerare con\n'
                         '  python scripts/genera_verbale_m2d.py\n')
        for riga in difflib.unified_diff(
                attuale.splitlines(), testo.splitlines(),
                fromfile=DESTINAZIONE, tofile='atteso', lineterm='', n=1):
            sys.stderr.write(riga + '\n')
        raise SystemExit(1)
    print('verbale M2d coerente con i grezzi')
    raise SystemExit(0)

io.open(DESTINAZIONE, 'w', encoding='utf-8', newline='\n').write(testo)
print('verbale scritto in %s' % DESTINAZIONE)
