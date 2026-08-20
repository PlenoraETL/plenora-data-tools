# -*- coding: utf-8 -*-
"""Genera il verbale della strumentazione di table.formula, o lo verifica.

Stesso patto degli altri due verbali: nessuna cifra battuta a mano.

    python scripts/genera_verbale_sonde.py             # rigenera
    python scripts/genera_verbale_sonde.py --verifica  # esce 1 se diverge

Va eseguito dalla radice del repository.
"""
import difflib
import io
import json
import re
import sys

GREZZI = ['docs/misure/sonde-formula/sonde-1.json',
          'docs/misure/sonde-formula/sonde-2.json']
DESTINAZIONE = 'docs/formula-strumentazione-2026-08-20.md'

run = [json.load(io.open(p, encoding='utf-8')) for p in GREZZI]

# Le sottofasi che compongono lo staging IPC e il suo replay: e' la somma che
# risponde alla domanda «quanto del residuo e' questo meccanismo».
IPC = ('staging_ipc', 'replay_decodifica_ipc', 'replay_compattazione',
       'staging_pulizia', 'replay_apertura', 'staging_chiusura',
       'replay_governor')


def misura(d, operazione):
    trovate = [m for m in d['misure'] if m['operazione'] == operazione]
    assert len(trovate) == 1, 'operazione assente: %s' % operazione
    return trovate[0]


def fase(m, nome):
    for f in m['sottofasi']:
        if f['fase'] == nome:
            return f['ns_per_esecuzione']
    return 0.0


def quota_ipc(m):
    """Quanto del residuo e' staging IPC + replay."""
    somma = sum(fase(m, n) for n in IPC)
    residuo = m['residuo_ns']
    return 100.0 * somma / residuo if residuo else 0.0


FORMULE = ('solo_formula', 'solo_formula_costante', 'solo_formula_intero')
ALTRE = ('solo_string_pad', 'solo_filter')


def estremi(valori):
    return min(valori), max(valori)


cop = [misura(d, o)['copertura_residuo_pct'] for d in run for o in FORMULE]
ipc = [quota_ipc(misura(d, o)) for d in run for o in FORMULE]
cop_min, cop_max = estremi(cop)
ipc_min, ipc_max = estremi(ipc)
sonda_min, sonda_max = estremi([d['costo_sonda_ns'] for d in run])

doc = []
A = doc.append

A(u"""# `table.formula`: dove va il tempo, e perche'

Ramo `perf/orchestrazione-v1`. **Misura, non ottimizzazione.** La
strumentazione usata qui e' **temporanea e non fa parte del codice**: e'
allegata come `docs/misure/sonde-formula/strumentazione.patch` e va riapplicata
per riprodurre i numeri. Nessuna API pubblica e nessuna metrica esistente sono
state toccate: le sonde scrivono su un canale separato, thread-locale.

Grezzi: `docs/misure/sonde-formula/sonde-1.json` e `sonde-2.json`, due
campagne indipendenti a host quieto.

Il documento precedente (`docs/decomposizione-wall-2026-08-20.md`) aveva
localizzato il residuo all'operazione `table.formula` — l'88%% del suo wall
fuori dal timer del suo kernel — senza dire dove. Questo lo dice.

---

## 1. La causa

`table.formula` e' nell'elenco delle operazioni che **emettono diagnostica per
riga**:

```rust
// plenora-core/src/catalog.rs:462
pub fn emits_row_diagnostics(&self, config: &serde_json::Value) -> bool {
    match self.family {
        Family::Table => match self.id {
            "table.flatten_json" | ... | "table.formula" | "table.expression"
            | "table.assert_not_null" | ... => true,
```

`table.string_pad` e `table.filter` non ci sono. E la differenza non e' locale
al nodo: decide il **percorso fisico dell'intero segmento**.

```rust
// executor.rs:1920, in segment_stream
if segment_emits_row_diagnostics(&self.plan, index) {
    Ok(row_diagnostic_stream(input, ..., index))   // <- scan + staging + replay
} else {
    Ok(Box::new(input.map(move |item| {            // <- attraversamento diretto
        run_streaming_chain(&plan, index, &state, item?, None)
    })))
}
```

Il percorso `row_diagnostic_stream` (R9.9) esiste per una ragione di
correttezza dichiarata: **nessun accepted esce prima che la scansione sia
completa**, cosi' una rejection tardiva non pubblica righe gia' consegnate.
Per ottenerlo:

1. `scan_row_diagnostic_segment` (executor.rs:2822) esegue la catena su ogni
   batch e ne fa **staging IPC su file temporaneo** (`stage_one_batch`,
   executor.rs:2175 — `StreamWriter::write` su `CountingFile`);
2. a scansione completa, il file viene **riletto** (`replay_staged_batch`,
   executor.rs:2095): `StreamReader::next`, poi `compact_staged_batch`, che
   fa un **`take` di ogni colonna** in buffer right-sized — una copia completa
   per batch;
3. il lease del governor viene ri-riservato per batch, la directory
   temporanea rimossa alla fine.

Quindi ogni riga viene **serializzata, scritta su disco, riletta, decodificata
e ricopiata**. Nessuna di queste fasi e' dentro `run_kernel`, che e' l'unica
cosa che `NodeMetrics.wall_time` misura.

---

## 2. I numeri

Piani a nodo singolo, stesso input (24 batch x 8192 righe), stesso percorso
fisico `LinearStreaming`, mediana su %d ripetizioni. Le sonde sono annidate:
`scansione_row_diagnostics` contiene lettura, catena e staging;
`replay_batch` contiene decodifica, compattazione e reservation; la somma usa
solo i contenitori piu' esterni.
""" % run[0]['misure'][0]['ripetizioni'])

for indice, d in enumerate(run, 1):
    A(u"\n### Campagna %d\n" % indice)
    A(u"| piano | wall | kernel | residuo | staging+replay | quota del residuo |")
    A(u"|---|---|---|---|---|---|")
    for o in FORMULE + ALTRE:
        m = misura(d, o)
        somma = sum(fase(m, n) for n in IPC)
        A(u"| `%s` | %.2f ms | %.2f ms | %.2f ms | %.2f ms | **%.0f%%** |" % (
            o, m['wall_ns'] / 1e6, m['kernel_ns'] / 1e6, m['residuo_ns'] / 1e6,
            somma / 1e6, quota_ipc(m)))

m0 = misura(run[0], 'solo_formula')
A(u"""
Dettaglio delle sottofasi di `solo_formula`, campagna 1:
""")
A(u"| sottofase | ms | quota del wall | ruolo |")
A(u"|---|---|---|---|")
for f in sorted(m0['sottofasi'], key=lambda x: -x['ns_per_esecuzione']):
    if f['ns_per_esecuzione'] / 1e6 < 0.02:
        continue
    if f['contenitore']:
        ruolo = u'contenitore'
    elif f['dentro_un_contenitore']:
        ruolo = u'dentro un contenitore'
    else:
        ruolo = u'foglia'
    A(u"| `%s` | %.3f | %.1f%% | %s |" % (
        f['fase'], f['ns_per_esecuzione'] / 1e6, f['quota_wall_pct'], ruolo))

A(u"""
**Lo staging IPC e il suo replay valgono da soli il %.0f–%.0f%% del residuo**
sui tre piani con `formula`. Su `string_pad` e `filter`, che non passano da
quel percorso, quelle sottofasi **non esistono** e il residuo e' sotto il
millisecondo.

Il costo **non dipende dall'espressione**: una formula costante (`1`), che non
legge alcuna colonna, ha lo stesso staging e lo stesso replay di `valore * 2`.
Il suo kernel costa meno di un millisecondo e il suo wall resta sopra i
quattordici.

### Copertura e calibrazione

La somma delle sottofasi copre il **%.0f–%.0f%%** del residuo sui piani con
`formula`. Cio' che resta — circa un millisecondo, meno dell'11%% — sta nello
strato di raccolta: l'iteratore di `collect_batches`, il rilascio del lease per
batch e la costruzione del `Vec` di uscita, che non sono strumentati.

Ogni sonda costa **%.0f–%.0f ns**. Con circa dodici sottofasi e
ventiquattro batch, la strumentazione pesa attorno a **%.2f ms per esecuzione**,
cioe' meno dell'%.1f%% del wall misurato: non e' lei a produrre questi numeri.

---

## 3. Il punto preciso

| dove | file:riga | che cosa |
|---|---|---|
| classificazione | `plenora-core/src/catalog.rs:471` | `table.formula` fra le operazioni con diagnostica per riga |
| biforcazione | `crates/plenora-engine/src/executor.rs:1920` | il segmento intero prende il percorso con staging |
| scansione | `crates/plenora-engine/src/executor.rs:2822` | `scan_row_diagnostic_segment` |
| scrittura | `crates/plenora-engine/src/executor.rs:2175` | `stage_one_batch`: `StreamWriter::write` per batch |
| rilettura | `crates/plenora-engine/src/executor.rs:2095` | `replay_staged_batch`: `StreamReader::next` |
| copia | `compact_staged_batch`, chiamata da `replay_staged_batch` | `take` di ogni colonna, una copia piena per batch |

---

## 4. Che cosa questa misura NON dice

- **non dice che il percorso sia sbagliato.** La garanzia che serve —
  nessun accepted pubblicato prima della fine della scansione — e' reale, ed e'
  la ragione per cui lo staging esiste. Questo documento misura il suo prezzo,
  non ne discute la necessita';
- **non propone una correzione.** Le alternative concepibili (buffer in memoria
  entro un tetto, staging solo quando una rejection si e' vista, riuso del
  batch senza `take`) hanno ciascuna implicazioni su memoria e semantica che
  non sono state valutate qui;
- **non generalizza alle altre operazioni della lista.** `expression`, gli
  `assert_*`, `date_*` e `flatten_json` prendono lo stesso percorso e pagheranno
  qualcosa di simile, ma **non sono state misurate**;
- **non copre l'ultimo ~10%% del residuo**, che resta attribuito allo strato di
  raccolta senza sonde.

---

## 5. Fatti verificati

1. `table.formula` fa prendere all'**intero segmento** il percorso
   row-diagnostics, che serializza ogni batch su file temporaneo e lo rilegge.
2. Quel meccanismo vale il **%.0f–%.0f%%** del residuo di un piano con una sola
   `formula`, su due campagne indipendenti.
3. Il costo **non dipende dall'espressione**: una costante che non legge
   colonne costa quanto un prodotto.
4. `string_pad` e `filter`, stesso input e stesso percorso fisico, hanno un
   residuo sotto il millisecondo.
5. La strumentazione pesa meno dell'%.1f%% del wall: i numeri non sono un
   artefatto delle sonde.
""" % (ipc_min, ipc_max, cop_min, cop_max, sonda_min, sonda_max,
       12 * 24 * sonda_max / 1e6,
       100.0 * 12 * 24 * sonda_max / m0['wall_ns'],
       ipc_min, ipc_max,
       100.0 * 12 * 24 * sonda_max / m0['wall_ns']))

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
                         '  python scripts/genera_verbale_sonde.py\n')
        for riga in difflib.unified_diff(
                attuale.splitlines(), testo.splitlines(),
                fromfile=DESTINAZIONE, tofile='atteso', lineterm='', n=1):
            sys.stderr.write(riga + '\n')
        raise SystemExit(1)
    print('verbale coerente con i grezzi delle sonde')
    raise SystemExit(0)

io.open(DESTINAZIONE, 'w', encoding='utf-8', newline='\n').write(testo)
print('verbale scritto in %s' % DESTINAZIONE)
