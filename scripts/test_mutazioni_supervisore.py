#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Le difese di `mutazioni_supervisore.py`, provate su un albero finto.

# Perche' esiste

Perche' quelle difese sono la ragione per cui il verdetto della campagna vale
qualcosa, e una difesa non provata e' una promessa. La campagna vera dura
mezz'ora e vuole una macchina Linux dedicata; questo giro dura secondi, non
compila niente, e si puo' eseguire ovunque — cosi' le garanzie restano
riproducibili senza il costo che le ha rese manuali.

# Come misura, senza toccare l'albero vero

Costruisce un albero **finto** dalla tabella dei mutanti stessa: per ogni file
citato scrive i suoi frammenti sani. Le mutazioni sono quindi applicabili
davvero, e la riparazione si puo' mettere alla prova su uno stato costruito
apposta. `MUTAZIONI_RADICE` punta lo script laggiu', e l'albero del repository
non viene mai aperto in scrittura.

# Uso

    python3 scripts/test_mutazioni_supervisore.py
"""
import importlib.util
import io
import os
import shutil
import subprocess
import sys
import tempfile

QUI = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(QUI, 'mutazioni_supervisore.py')

_specifica = importlib.util.spec_from_file_location('mutazioni', SCRIPT)
mutazioni = importlib.util.module_from_spec(_specifica)
_specifica.loader.exec_module(mutazioni)


def esegui(radice, *argomenti):
    """Lo script come lo esegue una persona: processo suo, codice di uscita."""
    ambiente = dict(os.environ, MUTAZIONI_RADICE=radice)
    finito = subprocess.run([sys.executable, SCRIPT, *argomenti],
                            capture_output=True, env=ambiente)
    return finito.returncode, finito.stdout.decode('utf-8', errors='replace')


def scrivi(percorso, testo):
    os.makedirs(os.path.dirname(percorso), exist_ok=True)
    with io.open(percorso, 'w', encoding='utf-8', newline='') as aperto:
        aperto.write(testo)


def costruisci_albero(radice):
    """Un file per ogni file citato dalla tabella, con dentro i suoi frammenti.

    Nell'albero vero due frammenti dello stesso file possono contenersi — quello
    della cancellazione comprende la riga del tempo scaduto — e li' ognuno
    compare comunque una volta sola. Concatenandoli qui, invece, il piu' corto
    comparirebbe due volte: si tiene percio' il solo piu' lungo di ogni catena
    di inclusione, e il contenuto vi compare una volta. E' un'imprecisione
    dell'albero finto, non della tabella.
    """
    per_file = {}
    for _, _, relativo, sano, _ in mutazioni.MUTANTI:
        per_file.setdefault(relativo, []).append(sano)
    for relativo, frammenti in per_file.items():
        soli = [f for f in frammenti
                if not any(f != altro and f in altro for altro in frammenti)]
        scrivi(os.path.join(radice, relativo), '\n// ---\n'.join(soli) + '\n')


#: I tre stati di una difesa. **Saltato non e' vinto**: un caso che non si e'
#: potuto eseguire — i gruppi di processi non esistono su questa piattaforma —
#: non ha misurato niente, e contarlo fra i verdi gonfierebbe il punteggio
#: esattamente come farebbe un mutante sparito dalla tabella. Sta fuori dal
#: numeratore **e** dal denominatore, e si dichiara a parte.
VINTO, PERSO, SALTATO = 'vinto', 'perso', 'saltato'


class Verbale:
    """Che cosa si e' preteso, come e' andata, e che cosa non si e' potuto fare."""

    def __init__(self):
        self.voci = []

    def esito(self, nome, riuscito, dettaglio='', uscita=''):
        self.voci.append((VINTO if riuscito else PERSO, nome, dettaglio, uscita))

    def salta(self, nome, motivo):
        """Una difesa che qui non si puo' eseguire, e che non si conta."""
        self.voci.append((SALTATO, nome, motivo, ''))

    def comando(self, nome, atteso, deve_contenere, codice, uscita):
        riuscito = codice == atteso and deve_contenere in uscita
        self.esito(nome, riuscito, f'codice {codice}, atteso {atteso}', uscita)

    def racconta(self):
        print()
        for stato, nome, dettaglio, uscita in self.voci:
            etichetta = {VINTO: 'ok     ', PERSO: 'ROSSO  ', SALTATO: 'saltato'}[stato]
            print(f'{etichetta} {nome}')
            if stato == SALTATO:
                print(f'         {dettaglio}')
            elif stato == PERSO:
                print(f'         {dettaglio}')
                for riga in uscita.splitlines()[:4]:
                    print('         | ' + riga.encode('ascii', 'replace').decode('ascii'))
        vinte = sum(1 for v in self.voci if v[0] == VINTO)
        saltate = sum(1 for v in self.voci if v[0] == SALTATO)
        eseguibili = len(self.voci) - saltate
        print()
        quante = 'una saltata' if saltate == 1 else f'{saltate} saltate'
        print(f'{vinte}/{eseguibili} difese eseguibili, {quante}')
        return vinte == eseguibili


def prova_gli_argomenti(verbale, radice):
    """Le forme che non devono passare.

    Un intervallo vuoto — `0 0`, `33 33`, gli estremi invertiti — eseguirebbe
    zero mutanti e finirebbe con successo: un verde che non ha misurato niente
    e' peggio di un rosso.
    """
    fuori = len(mutazioni.MUTANTI) + 1
    for nome, argomenti, atteso, testo in [
        ('intervallo 0 0 rifiutato', ('0', '0'), 1, 'intervallo non valido'),
        ('intervallo oltre il fondo rifiutato', (str(fuori), str(fuori)), 1,
         'intervallo non valido'),
        ('intervallo invertito rifiutato', ('5', '2'), 1, 'intervallo non valido'),
        ('argomenti di troppo rifiutati', ('1', '2', '3'), 1, 'argomenti di troppo'),
        ('estremo non canonico rifiutato', ('01', '2'), 1, 'forma canonica'),
        ('estremo con segno rifiutato', ('+1', '2'), 1, 'forma canonica'),
        ('estremo non numerico rifiutato', ('uno', '2'), 1, "non e' un numero"),
        ('prepara senza impronta rifiutato', ('--prepara',), 1, 'esattamente'),
        ('impronta con argomenti rifiutato', ('--impronta', 'extra'), 1,
         'non vuole altri argomenti'),
    ]:
        codice, uscita = esegui(radice, *argomenti)
        verbale.comando(nome, atteso, testo, codice, uscita)


def prova_l_impronta(verbale, radice):
    """Che identifichi l'albero, e che rifiuti il vuoto."""
    _, prima = esegui(radice, '--impronta')
    uno = os.path.join(radice, mutazioni.MACC)
    altro = os.path.join(radice, os.path.dirname(mutazioni.MACC), 'altro-nome.rs')
    os.rename(uno, altro)
    _, dopo = esegui(radice, '--impronta')
    os.rename(altro, uno)
    verbale.esito(
        "rinominare un file cambia l'impronta (stesso numero, stessi contenuti)",
        prima.strip() != dopo.strip(),
        f'{prima.strip()[:12]} contro {dopo.strip()[:12]}')

    prova_la_collisione(verbale)

    vuoto = tempfile.mkdtemp(prefix='albero-vuoto-')
    try:
        codice, uscita = esegui(vuoto, '--impronta')
        verbale.comando('una radice senza crates/ e\' rifiutata', 1,
                        'AlberoSenzaSorgenti', codice, uscita)
        os.makedirs(os.path.join(vuoto, 'crates'))
        codice, uscita = esegui(vuoto, '--impronta')
        verbale.comando('una radice senza sorgenti e\' rifiutata', 1,
                        'AlberoSenzaSorgenti', codice, uscita)
    finally:
        shutil.rmtree(vuoto, ignore_errors=True)


def flusso_senza_lunghezze(radice):
    """La vecchia codifica: percorso, un NUL, contenuto, e via il successivo.

    Serve a mostrare che cosa lascia passare. Non e' piu' nel codice: vive qui
    perche' una difesa si prova contro cio' che sostituisce.
    """
    pezzi = []
    for relativo in mutazioni.file_dei_sorgenti(radice):
        with open(os.path.join(radice, relativo), 'rb') as aperto:
            pezzi.append(relativo.encode('utf-8') + b'\0' + aperto.read())
    return b''.join(pezzi)


def prova_la_collisione(verbale):
    """**Due alberi diversi che la vecchia codifica non distingue.**

    Il primo ha due file; il secondo ne ha uno solo, il cui contenuto porta
    dentro di se' il percorso e il contenuto del secondo file del primo, NUL
    compreso. Concatenando percorso, NUL e contenuto senza confini strutturali i
    due producono **lo stesso identico flusso di byte**: la fine di un contenuto
    e l'inizio del percorso successivo si spartiscono diversamente, e nessuno se
    ne accorge.

    Un separatore non basta a chiudere questa strada, perche' il contenuto puo'
    contenerlo — qui infatti lo contiene. La lunghezza prefissata la chiude,
    perche' dice **quanti byte** leggere prima di passare al pezzo dopo.
    """
    base = tempfile.mkdtemp(prefix='prova-collisione-')
    try:
        due = os.path.join(base, 'due')
        scrivi(os.path.join(due, 'crates', 'a.rs'), 'Z')
        scrivi(os.path.join(due, 'crates', 'b.rs'), 'W')

        uno = os.path.join(base, 'uno')
        scrivi(os.path.join(uno, 'crates', 'a.rs'),
               'Z' + 'crates/b.rs' + '\0' + 'W')

        verbale.esito(
            'la vecchia codifica dava lo stesso flusso a due alberi diversi',
            flusso_senza_lunghezze(due) == flusso_senza_lunghezze(uno),
            'i due flussi differiscono: il caso non misura piu\' niente')

        con_due = mutazioni.impronta(due)
        con_uno = mutazioni.impronta(uno)
        verbale.esito(
            'le lunghezze prefissate li distinguono',
            con_due != con_uno, f'{con_due[:12]} contro {con_uno[:12]}')
    finally:
        shutil.rmtree(base, ignore_errors=True)


def prova_il_nipote_superstite(verbale, radice):
    """**Un nipote vivo dopo un'uscita ordinaria viene trovato e tolto.**

    E' la classe di difetto che questi mutanti cercano nel supervisore, e sarebbe
    assurdo che l'harness la lasciasse passare su di se': `cargo` che finisce
    ordinatamente non dice niente sui nipoti.

    Il comando finto fa proprio questo — genera un `sleep`, ne scrive il pid, ed
    esce con successo lasciandolo nel proprio gruppo. Senza il controllo dopo
    ogni uscita, quel `sleep` resterebbe vivo per tutta la campagna.

    Solo su POSIX: i gruppi di processi sono quelli. Altrove il caso si
    **salta**, e il riepilogo lo dice: un caso che non si e' potuto eseguire non
    ha misurato niente, e contarlo fra i verdi sarebbe la stessa specie di
    inflazione che l'elenco canonico dei mutanti esiste per impedire.
    """
    if os.name != 'posix':
        verbale.salta('il nipote superstite viene trovato e tolto',
                      'i gruppi di processi sono di POSIX, e qui non ci sono')
        return

    dove = os.path.join(radice, 'pid-del-nipote')
    comando, radice_vera, uscita_vera = (
        mutazioni.COMANDO_BASE, mutazioni.RADICE, mutazioni.USCITA)
    try:
        # Anche la radice e il file d'uscita puntano all'albero finto: il caso
        # non deve dipendere dall'esistenza di quello vero.
        mutazioni.RADICE = radice
        mutazioni.USCITA = os.path.join(radice, 'uscita-del-caso.txt')
        mutazioni.COMANDO_BASE = [
            'sh', '-c', f'sleep 300 & echo $! > {dove}; exit 0', '--',
        ]
        codice, raccolto = mutazioni.esegui([])
        with io.open(dove, encoding='utf-8') as aperto:
            nipote = int(aperto.read().strip())
    finally:
        (mutazioni.COMANDO_BASE, mutazioni.RADICE, mutazioni.USCITA) = (
            comando, radice_vera, uscita_vera)

    verbale.esito('il comando finto esce con successo', codice == 0, f'codice {codice}')
    verbale.esito('e il gruppo risulta comunque raccolto', raccolto)
    verbale.esito(
        'perche\' il nipote e\' stato trovato e tolto',
        not os.path.exists(f'/proc/{nipote}'),
        f'il pid {nipote} e\' ancora li\'')


def prova_la_baseline(verbale, radice):
    """Che si verifichi, e che resti dov'e' invece di essere rimpiazzata."""
    codice, uscita = esegui(radice, '--prepara', 'a' * 64)
    verbale.comando('prepara con impronta sbagliata rifiuta', 1,
                    "NON E' QUELLO DICHIARATO", codice, uscita)

    _, vera = esegui(radice, '--impronta')
    firma = vera.strip()
    codice, uscita = esegui(radice, '--prepara', firma)
    verbale.comando('prepara con impronta giusta fissa la baseline', 0,
                    'baseline fissata', codice, uscita)

    codice, uscita = esegui(radice, '--prepara', firma)
    verbale.comando('prepara due volte verifica e non sovrascrive', 0,
                    "gia' presente, verificata", codice, uscita)

    copia = os.path.join(radice + '-baseline', mutazioni.MACC)
    os.chmod(copia, 0o644)
    originale = io.open(copia, encoding='utf-8').read()
    scrivi(copia, originale + '\n// alterazione\n')
    codice, uscita = esegui(radice, '1', '1')
    verbale.comando('baseline alterata ferma il giro', 2, 'BASELINE ALTERATA',
                    codice, uscita)
    scrivi(copia, originale)


def prova_la_riparazione(verbale, radice):
    """Che riconosca, che ripari, e che non tocchi cio' che non riconosce."""
    bersaglio = os.path.join(radice, mutazioni.MACC)
    sano = io.open(bersaglio, encoding='utf-8').read()

    estraneo = sano + '\n// una modifica che nessun mutante spiega\n'
    scrivi(bersaglio, estraneo)
    codice, uscita = esegui(radice, '1', '1')
    verbale.comando('stato non riconosciuto ferma il giro', 2,
                    'ALBERO NON RICONOSCIUTO', codice, uscita)
    verbale.esito('e il file estraneo non viene sovrascritto',
                  io.open(bersaglio, encoding='utf-8').read() == estraneo)
    scrivi(bersaglio, sano)

    identificativo, _, relativo, frammento, mutato = mutazioni.MUTANTI[0]
    percorso = os.path.join(radice, relativo)
    testo = io.open(percorso, encoding='utf-8').read()
    scrivi(percorso, testo.replace(frammento, mutato))
    codice, uscita = esegui(radice, '1', '1')
    verbale.esito('una mutazione nota si riconosce e si ripara',
                  f'riparato: restava {identificativo}' in uscita,
                  f'codice {codice}', uscita)
    verbale.esito('e il file torna alla baseline',
                  io.open(percorso, encoding='utf-8').read() == testo)


def prova_i_preflight(verbale):
    """Che l'elenco sia canonico, e che i trentadue alberi siano distinti.

    Si chiamano in diretta invece che dal processo, perche' cio' che si vuole
    provare qui e' la regola — e la regola la si legge meglio dal suo valore di
    ritorno che da una riga di testo.
    """
    verbale.esito("l'elenco della tabella e' canonico",
                  mutazioni.elenco_e_canonico())
    verbale.esito('i mutanti sono trentadue',
                  len(mutazioni.MUTANTI) == len(mutazioni.IDENTIFICATORI) == 32,
                  f'{len(mutazioni.MUTANTI)} nella tabella')


def prova_la_distinzione(verbale, radice):
    """Due mutanti che producono lo stesso albero fermano il giro.

    Si costruisce il caso invece di sperarlo: si duplica il primo mutante con un
    identificativo suo, e si pretende che il preflight lo dica.
    """
    attesa = mutazioni.impronta(radice)
    verbale.esito('sull\'albero finto i trentadue alberi mutati sono distinti',
                  mutazioni.tutti_i_mutanti_sono_distinti(radice, attesa))

    tabella = list(mutazioni.MUTANTI)
    identificativo, nome, relativo, sano, malato = tabella[0]
    tabella.append(('M99', nome + ' (copia)', relativo, sano, malato))
    originale = mutazioni.MUTANTI
    try:
        mutazioni.MUTANTI = tabella
        verbale.esito('due mutanti con lo stesso albero vengono rifiutati',
                      not mutazioni.tutti_i_mutanti_sono_distinti(radice, attesa))
        # E un mutante che non muta niente: la baseline stessa.
        mutazioni.MUTANTI = [(identificativo, nome, relativo, sano, sano)]
        verbale.esito('un mutante che non muta niente viene rifiutato',
                      not mutazioni.tutti_i_mutanti_sono_distinti(radice, attesa))
    finally:
        mutazioni.MUTANTI = originale


def principale():
    base = tempfile.mkdtemp(prefix='prova-mutazioni-')
    radice = os.path.join(base, 'albero')
    costruisci_albero(radice)
    verbale = Verbale()
    try:
        prova_i_preflight(verbale)
        prova_gli_argomenti(verbale, radice)
        prova_l_impronta(verbale, radice)
        prova_la_distinzione(verbale, radice)
        prova_il_nipote_superstite(verbale, radice)
        prova_la_baseline(verbale, radice)
        prova_la_riparazione(verbale, radice)
    finally:
        shutil.rmtree(base, ignore_errors=True)
        shutil.rmtree(radice + '-baseline', ignore_errors=True)
    return 0 if verbale.racconta() else 1


if __name__ == '__main__':
    sys.exit(principale())
