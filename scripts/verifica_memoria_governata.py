# -*- coding: utf-8 -*-
"""Verifica che la memoria governata sia ancora quella descritta.

`docs/errori-e-limiti.md` dichiara che cosa il budget governato garantisce e
dove la garanzia si ferma: quali siti prenotano PRIMA di allocare, quali
prenotano dopo, e quale primitivo esiste per prendere la quota. Un documento
del genere marcisce in silenzio — basta che qualcuno sposti una reservation e
il testo resta convincente e falso.

Questo script non conta le righe, che cambiano al primo refactor: verifica che
ogni sito descritto esista ancora **nella forma in cui e' descritto**, e che i
pattern che sono stati eliminati non riappaiano. Esce 1 al primo scostamento.

Tiene solo cio' che un test non fissa: l'ordine fra allocazione e prenotazione
dei siti che il documento dichiara «dopo», e i pattern eliminati
dell'esecutore. I comportamenti del governor e della consegna li fissano i test
di `governor.rs` (permesso, ritaglio, collisione di id, nascita mancante, eta'
del lease piu' vecchio, snapshot, contatori esauriti, `verifica_salute`) e di
`executor/tests.rs` (`iterator_pubblico_intercetta_la_contabilita_corrotta`,
`iterator_corrotto_a_meta_stream_emette_una_sola_volta`,
`metriche_parziali_dichiarano_la_contabilita_corrotta`,
`collect_batches_e_publish_restano_protetti`): un'ancora testuale li
duplicherebbe, e un test verifica che cosa succede, non come e' scritto.

    python scripts/verifica_memoria_governata.py
"""
import io
import sys

ESECUTORE = 'crates/plenora-engine/src/executor.rs'
# La consegna dell'output vive in un modulo proprio dell'esecutore: le
# garanzie sono quelle dell'esecutore, il file no.
USCITA = 'crates/plenora-engine/src/executor/output.rs'
STAGING = 'crates/plenora-engine/src/executor/staging.rs'
DIAGNOSTICA = 'crates/plenora-engine/src/executor/diagnostics.rs'
GEO = 'crates/plenora-engine/src/executor/geo.rs'
BLOCKING = 'crates/plenora-engine/src/executor/blocking.rs'
FUSIONE = 'crates/plenora-engine/src/executor/fusion.rs'
STREAMING = 'crates/plenora-engine/src/executor/streaming.rs'

# (file, frammento, che cosa dimostra la sua presenza)
ANCORE = [
    (ESECUTORE,
     '.reserve(bytes, &edge_name)',
     'arco di ingresso: il batch esiste gia\' quando lo si prenota'),
    (BLOCKING,
     'concat_batches(&schema, &unwrapped)',
     'blocking unario: la materializzazione precede check e reservation'),
    (GEO,
     'concat_batches(&right_schema, &unwrapped)',
     'blocking binario: idem, sui due lati'),
    (GEO,
     '.reserve(output.get_array_memory_size() as u64, &kernel.node_id)',
     'uscita dei nodi blocking: il kernel ha gia\' allocato'),
    (FUSIONE,
     'let Some(decoded_bytes) = fused_group_decoded_bytes(batch, &kernels[0])',
     'gruppo geo fuso: stima dai payload di input, PRIMA di decodificare'),
    (FUSIONE,
     '.try_reserve(decoded_bytes, &kernels[0].node_id)',
     'gruppo geo fuso: la reservation precede l\'allocazione'),
    (DIAGNOSTICA,
     'fn permesso_di_trattenere(',
     'segmenti row-diagnostics: permesso PRIMA della passata'),
    (STREAMING,
     'permesso.ritaglia(bytes_at_boundary)',
     'l\'uscita si ritaglia dal permesso invece di riprenotare'),
    (STAGING,
     'match replay.reader.next()',
     'replay dello staging: la decodifica precede la ri-riserva'),
    (STREAMING,
     'permesso.ritaglia(bytes_at_boundary)?',
     'l\'uscita si ritaglia, e un ritaglio fallito propaga invece di ripiegare'),
    (USCITA,
     '.or_else(|| self.state.verifica_heartbeat().err())',
     'il consumo per iteratore ha il controllo terminale: senza, una '
     'corruzione rilevata nell\'ultimo Drop sarebbe un successo silenzioso. '
     'Il terminale riporta ora anche un heartbeat fermo da oltre la '
     'tolleranza, che renderebbe la directory raccoglibile mentre lo '
     'stream si chiude dichiarando successo'),
]

# Frammenti che NON devono ricomparire: sono i pattern che questo blocco ha
# eliminato. Se tornano, docs/errori-e-limiti.md va riletto.
VIETATI = [
    (ESECUTORE,
     'fn accedibile_in_memoria(',
     'la soglia a due operazioni e\' stata sostituita dal permesso'),
    (ESECUTORE,
     'accepted.trattenuti()',
     'il contatore locale dei byte trattenuti e\' stato eliminato'),
    (ESECUTORE,
     'Some(lease) => lease,',
     'era il ripiego da ritaglio fallito a nuova reserve: riapriva il TOCTOU '
     'che il permesso esiste per chiudere'),
]


def testo(percorso):
    return io.open(percorso, encoding='utf-8', newline='').read()


def main():
    problemi = []
    cache = {}
    for percorso, frammento, motivo in ANCORE:
        if percorso not in cache:
            cache[percorso] = testo(percorso)
        if frammento not in cache[percorso]:
            problemi.append(
                'ANCORA PERSA in %s: %r\n  il documento afferma: %s'
                % (percorso, frammento, motivo))
    for percorso, frammento, motivo in VIETATI:
        if percorso not in cache:
            cache[percorso] = testo(percorso)
        if frammento in cache[percorso]:
            problemi.append(
                'PATTERN RIAPPARSO in %s: %r\n  %s'
                % (percorso, frammento, motivo))
    if problemi:
        sys.stderr.write("la memoria governata non e' quella descritta in "
                         'docs/errori-e-limiti.md:\n\n')
        for problema in problemi:
            sys.stderr.write('- %s\n' % problema)
        sys.stderr.write(
            '\nRileggere docs/errori-e-limiti.md prima di '
            'aggiornarlo: un elenco sbagliato e\' peggio di nessun elenco.\n')
        raise SystemExit(1)
    print('memoria governata coerente con docs/errori-e-limiti.md: '
          '%d ancore, %d pattern eliminati e non riapparsi'
          % (len(ANCORE), len(VIETATI)))


main()
