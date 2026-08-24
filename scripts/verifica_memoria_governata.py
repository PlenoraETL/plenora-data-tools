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

    python scripts/verifica_memoria_governata.py
"""
import io
import sys

ESECUTORE = 'crates/plenora-engine/src/executor.rs'
# La consegna dell'output vive in un modulo proprio dal 2026-08-24
# (PR 2 del refactor strutturale): le garanzie sono le stesse, il file no.
USCITA = 'crates/plenora-engine/src/executor/output.rs'
GOVERNOR = 'crates/plenora-engine/src/governor.rs'

# (file, frammento, che cosa dimostra la sua presenza)
ANCORE = [
    (ESECUTORE,
     '.reserve(bytes, &edge_name)',
     'arco di ingresso: il batch esiste gia\' quando lo si prenota'),
    (ESECUTORE,
     'concat_batches(&schema, &unwrapped)',
     'blocking unario: la materializzazione precede check e reservation'),
    (ESECUTORE,
     'concat_batches(&right_schema, &unwrapped)',
     'blocking binario: idem, sui due lati'),
    (ESECUTORE,
     '.reserve(output.get_array_memory_size() as u64, &kernel.node_id)',
     'uscita dei nodi blocking: il kernel ha gia\' allocato'),
    (ESECUTORE,
     'let Some(decoded_bytes) = fused_group_decoded_bytes(batch, &kernels[0])',
     'gruppo geo fuso: stima dai payload di input, PRIMA di decodificare'),
    (ESECUTORE,
     '.try_reserve(decoded_bytes, &kernels[0].node_id)',
     'gruppo geo fuso: la reservation precede l\'allocazione'),
    (ESECUTORE,
     'fn permesso_di_trattenere(',
     'segmenti row-diagnostics: permesso PRIMA della passata'),
    (ESECUTORE,
     'permesso.ritaglia(bytes_at_boundary)',
     'l\'uscita si ritaglia dal permesso invece di riprenotare'),
    (ESECUTORE,
     'match replay.reader.next()',
     'replay dello staging: la decodifica precede la ri-riserva'),
    (GOVERNOR,
     'pub fn permesso(&self, bytes: u64, owner: &str) -> Result<Option<MemoryPermit>>',
     'il permesso atomico esiste, ed e\' il primitivo'),
    (GOVERNOR,
     'stato: Mutex<Contabilita>',
     'la contabilita\' sta sotto un lock unico: snapshot linearizzabile'),
    (GOVERNOR,
     'births.values().min().map(Instant::elapsed)',
     'il lease piu\' vecchio si sceglie per istante minimo, non per id'),
    (GOVERNOR,
     'pub fn ritaglia(self, bytes: u64) -> Result<MemoryLease>',
     'il ritaglio e\' fallibile: nessun ripiego su una nuova prenotazione'),
    (GOVERNOR,
     'pub fn in_lease(self) -> Result<MemoryLease>',
     'nessuna trasformazione del permesso riesce su governor corrotto'),
    (GOVERNOR,
     'pub fn verifica_salute(&self, owner: &str) -> Result<()>',
     'il cancello prima di dichiarare conclusa un\'esecuzione'),
    (GOVERNOR,
     'if stato.births.contains_key(&id)',
     'la collisione di id si verifica PRIMA di mutare: mai prenotazioni parziali'),
    (GOVERNOR,
     'if stato.births.remove(&self.id).is_none()',
     'una nascita mancante al rilascio marca la contabilita\''),
    (USCITA,
     'self.state.governor.verifica_salute("output")?;',
     'la consegna dell\'output passa dal controllo di salute'),
    (ESECUTORE,
     'permesso.ritaglia(bytes_at_boundary)?',
     'l\'uscita si ritaglia, e un ritaglio fallito propaga invece di ripiegare'),
    (USCITA,
     '.or_else(|| self.state.verifica_heartbeat().err())',
     'il consumo per iteratore ha il controllo terminale: senza, una '
     'corruzione rilevata nell\'ultimo Drop sarebbe un successo silenzioso. '
     'Il terminale riporta ora anche un heartbeat fermo da oltre la '
     'tolleranza, che renderebbe la directory raccoglibile mentre lo '
     'stream si chiude dichiarando successo'),
    (USCITA,
     'if self.esaurito {',
     'lo stato terminale impedisce di ripetere l\'errore a ogni chiamata'),
    (GOVERNOR,
     'pub accounting_corrupted: bool',
     'la corruzione e\' visibile nelle metriche parziali, che sono pubbliche'),
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
    (GOVERNOR,
     'births.values().next()',
     'sceglieva il lease per id minore invece che per istante minimo: '
     'sotto concorrenza l\'id e\' assegnato prima della lettura dell\'orologio'),
    (GOVERNOR,
     'live_leases.fetch_add',
     'incremento non controllato del contatore dei lease vivi'),
    (GOVERNOR,
     'next_lease_id.fetch_add',
     'incremento non controllato del generatore di id'),
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
