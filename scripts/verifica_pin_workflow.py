# -*- coding: utf-8 -*-
"""Gate: ogni action dei workflow e' riferita a una SHA completa.

`docs/release.md` dichiara che la CI e' riproducibile: lo stesso commit deve
essere verificato dallo stesso codice. Un `uses: owner/action@v4` non lo
garantisce — `v4` e' un tag mobile, e chi lo controlla puo' spostarlo su
qualunque commit fra due esecuzioni della stessa PR. Vale anche per i rami
usati come selettori (`dtolnay/rust-toolchain@1.92.0` e' un RAMO, non un
tag di versione).

Il gate pretende quindi, per ogni `uses:` che riferisca un'action su GitHub:

- un riferimento di 40 cifre esadecimali (SHA-1 completa). Una SHA
  abbreviata non basta: e' ambigua per costruzione;
- un commento sulla stessa riga che dica quale versione quella SHA
  rappresenta, altrimenti il pin diventa illeggibile e nessuno lo aggiorna
  piu'.

Sono esenti le action locali (`./...`) e i container Docker (`docker://`),
che non hanno un riferimento git.

Come gli altri gate del progetto si autoverifica: inietta forme che deve
rifiutare e forme che deve lasciar passare.

    python scripts/verifica_pin_workflow.py
"""
import io
import os
import re
import sys

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORKFLOW = os.path.join(RADICE, '.github', 'workflows')

# Una voce di mapping YAML in stile blocco: `chiave: valore`, con la chiave
# eventualmente quotata e uno spazio facoltativo prima dei due punti — tutte
# forme valide che una regex sulla sola `uses:` non quotata non vede.
VOCE = re.compile(
    r'^\s*(?:-\s+)?(?P<chiave>"[^"]*"|\'[^\']*\'|[^:\s#][^:#]*?)\s*:'
    r'(?:\s+(?P<valore>.*))?$')

# Espressione di GitHub Actions: compare ovunque nei workflow e non e' una
# collezione in stile flow. Va neutralizzata prima di cercarne una vera,
# altrimenti ogni riga con `${{ ... }}` risulterebbe non analizzabile.
ESPRESSIONE_ACTIONS = re.compile(r'\$\{\{.*?\}\}')

# Apertura di una collezione in stile flow. Se ne compare una su una riga che
# nomina anche `uses`, il gate FALLISCE invece di tacere: una flow mapping non
# e' analizzabile riga per riga, e cercare la chiave con una regex si e' gia'
# rotto due volte — su una graffa dentro una stringa (`{ name: "}", uses: … }`)
# e su una collezione annidata prima della chiave (`{ env: { X: y }, uses: … }`).
# Qui non si tenta piu' di capirla: la si rifiuta.
FLOW_APERTA = re.compile(r'[\[{]')
PAROLA_USES = re.compile(r'\buses\b')

# Chiave esplicita YAML (`? uses` su una riga, `: valore` sulla successiva):
# forma valida e rarissima, che questo gate non analizza — quindi rifiuta.
CHIAVE_ESPLICITA = re.compile(r'^\s*(?:-\s+)?\?(?:\s|$)')

# Anchor (`&nome`) e alias (`*nome`) di YAML. Un alias puo' fare da CHIAVE:
# `name: &k uses` e poi `- *k: action@v4` risolve a una chiave `uses` che il
# confronto letterale non vede. Non si prova a risolverli — servirebbe un
# parser YAML completo — si rifiutano: in questi workflow non se ne usano, e
# rifiutare una forma che nessuno scrive non costa nulla.
#
# Il nome dell'anchor NON e' un identificatore in stile linguaggio di
# programmazione: YAML ammette `&1`, `&-x` e in generale qualunque sequenza
# senza spazi. Restringere il dominio a lettere e underscore lascerebbe
# passare `&1`, che e' YAML validissimo.
#
# Conta invece la POSIZIONE: un anchor sta all'inizio di un nodo — a capo,
# dopo un trattino di sequenza, dopo i due punti di una chiave, o dopo
# un'apertura di collezione. Senza il vincolo di posizione, una `&` in mezzo
# a un comando di shell (`cargo a && cargo b`) verrebbe scambiata per un
# anchor.
ANCHOR_O_ALIAS = re.compile(r'(?:^\s*(?:-\s+)?|:\s+|[\[{,]\s*)[&*]\S')

# Tag YAML (`!!str`, `!qualcosa`) in posizione strutturale. Il parser di
# GitHub Actions legge il valore RISOLTO di uno scalare, quindi
# `- !!str uses: action@v4` ha davvero la chiave `uses`, mentre il confronto
# testuale vede `!!str uses`. Come per gli anchor: non si risolve, si
# rifiuta. Il vincolo di posizione evita di scambiare per tag i punti
# esclamativi dentro un comando di shell (`if ! test -f x`).
TAG_YAML = re.compile(r'(?:^\s*(?:-\s+)?|:\s+|[\[{,]\s*)!\S')

# Apertura di uno scalare a blocco (`run: |`, `description: >-`, …). Tutto
# cio' che segue, piu' indentato, e' TESTO — tipicamente uno script di shell —
# e non struttura YAML: cercarci chiavi, tag o anchor produce falsi positivi
# garantiti, perche' `*[!0-9]*` in uno script e' un pattern di glob e non un
# tag, e un `uses:` dentro un comando non e' una dichiarazione.
APRE_BLOCCO = re.compile(
    r'^(?P<indent>\s*)(?P<trattino>-\s+)?[^:#]*:\s*[|>][-+0-9]*\s*(?:#.*)?$')

SHA_COMPLETA = re.compile(r'^[0-9a-f]{40}$')
DIGEST_DOCKER = re.compile(r'@sha256:[0-9a-f]{64}$')


def senza_commento(valore):
    """Valore YAML senza il commento finale, piu' il commento stesso."""
    fuori_apice = True
    for indice, carattere in enumerate(valore):
        if carattere in '"\'':
            fuori_apice = not fuori_apice
        elif carattere == '#' and fuori_apice and (indice == 0 or valore[indice - 1].isspace()):
            return valore[:indice].strip(), valore[indice:].strip()
    return valore.strip(), ''


def senza_apici(testo):
    if len(testo) >= 2 and testo[0] == testo[-1] and testo[0] in '"\'':
        return testo[1:-1]
    return testo


def neutralizza_stringhe(riga):
    """Sostituisce il contenuto dei letterali con `.`, stessa lunghezza.

    Serve a cercare la struttura della riga senza che una graffa o una
    parentesi dentro una stringa cambi il risultato.
    """
    fuori = list(riga)
    apice = None
    for indice, carattere in enumerate(riga):
        if apice is None:
            if carattere in '"\'':
                apice = carattere
            continue
        if carattere == apice:
            apice = None
            continue
        fuori[indice] = '.'
    return ''.join(fuori)


def voci_uses(testo):
    """(riga, valore, commento) per ogni voce la cui chiave e' `uses`.

    Il secondo elemento della tupla e' `None` per le forme che il gate non sa
    analizzare: chi chiama le tratta come violazioni, non come assenze.
    """
    trovate = []
    indent_blocco = None
    for numero, riga in enumerate(testo.split('\n'), start=1):
        # Contenuto di uno scalare a blocco: testo, non YAML. Si salta
        # interamente finche' l'indentazione non torna al livello che lo ha
        # aperto.
        if indent_blocco is not None:
            if not riga.strip():
                continue
            if len(riga) - len(riga.lstrip()) > indent_blocco:
                continue
            indent_blocco = None
        apertura = APRE_BLOCCO.match(riga)
        if apertura is not None:
            # L'indentazione che conta e' quella della CHIAVE, non quella del
            # trattino di sequenza. In `- name: |` la chiave `name` sta due
            # colonne piu' a destra del trattino, e le chiavi sorelle della
            # stessa voce — `uses` compresa — stanno alla sua colonna.
            # Misurando dal trattino, una `uses:` sorella risulterebbe piu'
            # indentata del blocco e verrebbe scambiata per il suo
            # contenuto: un riferimento mobile passerebbe senza essere
            # guardato.
            trattino = apertura.group('trattino') or ''
            indent_blocco = len(apertura.group('indent')) + len(trattino)
            # I rifiuti strutturali valgono anche per la chiave che APRE il
            # blocco: `- "uses": >-` e' YAML valido, risolve alla
            # chiave `uses`, e finire nel ramo del blocco prima di
            # controllarla significherebbe saltare la riga e tutto il
            # contenuto.
            struttura_apertura = neutralizza_stringhe(
                ESPRESSIONE_ACTIONS.sub('.', senza_commento(riga)[0]))
            if (ANCHOR_O_ALIAS.search(struttura_apertura)
                    or TAG_YAML.search(struttura_apertura)
                    or '\\' in riga):
                trovate.append((numero, None, None))
                continue
            # Se la chiave che apre il blocco e' proprio `uses`, il
            # riferimento sta nel contenuto e questo gate non lo risolve:
            # `uses: >-` seguito da `actions/checkout@v4` e' uno scalare
            # valido che vale esattamente quel riferimento. Saltare la riga
            # perche' apre un blocco significherebbe non guardare mai la
            # chiave.
            corpo = senza_commento(riga)[0]
            chiave = corpo.split(':', 1)[0].strip()
            if chiave.startswith('- '):
                chiave = chiave[2:].strip()
            if senza_apici(chiave) == 'uses':
                trovate.append((numero, None, None))
            continue
        # I controlli strutturali guardano il solo CODICE: un commento puo'
        # contenere qualunque cosa — enfasi con asterischi, graffe, la parola
        # `uses` — e non e' YAML. Il commento serve piu' avanti, come
        # marcatore di versione del pin, e li' viene riletto dal valore.
        codice = senza_commento(riga)[0]
        # Ordine: prima le espressioni di Actions (che usano le graffe e non
        # sono collezioni), poi le stringhe (che possono contenere graffe).
        senza_espressioni = ESPRESSIONE_ACTIONS.sub('.', codice)
        struttura = neutralizza_stringhe(senza_espressioni)
        # La parola si cerca sul testo CON le stringhe: in `{ "uses": ... }`
        # la chiave e' essa stessa quotata, e cercarla sulla struttura
        # neutralizzata la farebbe sparire — la riga risulterebbe una flow
        # mapping che non nomina `uses`, e non verrebbe esaminata da nessuno
        # dei due rami.
        if FLOW_APERTA.search(struttura) and PAROLA_USES.search(senza_espressioni):
            trovate.append((numero, None, None))
            continue
        if CHIAVE_ESPLICITA.match(struttura):
            trovate.append((numero, None, None))
            continue
        if ANCHOR_O_ALIAS.search(struttura) or TAG_YAML.search(struttura):
            trovate.append((numero, None, None))
            continue
        corrispondenza = VOCE.match(riga)
        if corrispondenza is None:
            continue
        chiave = corrispondenza.group('chiave').strip()
        # Una chiave con escape (`"uses"`) e' YAML valido e denota la
        # stessa chiave: senza decodificarla il confronto testuale la
        # mancherebbe. Non si decodifica — si rifiuta, che e' il verso
        # prudente e non richiede di reimplementare l'escaping di YAML.
        if '\\' in chiave:
            trovate.append((numero, None, None))
            continue
        if senza_apici(chiave) != 'uses':
            continue
        grezzo = corrispondenza.group('valore')
        if grezzo is None:
            trovate.append((numero, None, None))
            continue
        valore, commento = senza_commento(grezzo)
        trovate.append((numero, senza_apici(valore), commento))
    return trovate


def problemi_nel_testo(nome, testo):
    """Elenco di (file, riga, motivo) per ogni `uses` non conforme."""
    trovati = []
    for numero, riferimento, commento in voci_uses(testo):
        if riferimento is None:
            trovati.append((
                numero,
                'forma di `uses` non analizzabile da questo gate: '
                'riscriverla come mapping a blocco'))
            continue
        # Le action locali vivono nel repository e sono gia' alla revisione
        # verificata: non hanno un riferimento git proprio.
        if riferimento.startswith('./'):
            continue
        # Un'immagine Docker e' pinnata solo dal digest: `alpine:3.20` e'
        # mobile esattamente come un tag git.
        if riferimento.startswith('docker://'):
            if not DIGEST_DOCKER.search(riferimento):
                trovati.append((
                    numero,
                    '%s: immagine senza digest `@sha256:...` (i tag Docker '
                    'sono mobili)' % riferimento))
            continue
        if '@' not in riferimento:
            trovati.append((numero, '%s: nessun riferimento dopo `@`' % riferimento))
            continue
        action, _, versione = riferimento.rpartition('@')
        if not SHA_COMPLETA.match(versione):
            trovati.append((
                numero,
                '%s: `%s` non e\' una SHA completa (tag e rami sono mobili)'
                % (action, versione)))
            continue
        if not commento:
            trovati.append((
                numero,
                '%s: SHA senza commento di versione, il pin diventa illeggibile'
                % action))
    return [(nome, numero, motivo) for numero, motivo in trovati]


def sorgenti_da_esaminare():
    """Workflow **e** composite action locali del repository.

    I workflow non sono l'unico posto da cui GitHub esegue una action: un
    `uses: ./.github/actions/x` e' esente qui — sta nel repository, alla
    revisione verificata — ma il suo `action.yml` puo' a sua volta contenere
    `uses: owner/action@v4`, e quello e' mobile. Leggere solo i workflow
    lascerebbe quel riferimento fuori dal perimetro del gate.
    """
    trovati = []
    if os.path.isdir(WORKFLOW):
        for nome in sorted(os.listdir(WORKFLOW)):
            if nome.endswith('.yml') or nome.endswith('.yaml'):
                trovati.append(os.path.join(WORKFLOW, nome))
    # Composite action: ogni `action.yml`/`action.yaml` TRACCIATO.
    #
    # L'elenco viene da `git ls-files`, non da una passeggiata con potatura
    # per nome: potare le directory che cominciano per `target` escluderebbe
    # anche `.github/target-action/action.yml`, che e' un manifesto
    # legittimo e tracciato — un workflow puo' usarlo come action locale, e
    # il suo `uses` interno resterebbe invisibile. Cio' che e' tracciato
    # fa parte della revisione verificata, ed e' esattamente il perimetro
    # giusto.
    import subprocess
    try:
        elenco = subprocess.run(
            ['git', 'ls-files', '-z', '*action.yml', '*action.yaml'],
            cwd=RADICE, capture_output=True, check=True)
        tracciati = [n for n in elenco.stdout.decode('utf-8').split('\0') if n]
    except (OSError, subprocess.CalledProcessError):
        # Fuori da un checkout git si ricade sulla passeggiata, senza
        # potature per nome: meglio esaminare di piu' che saltare qualcosa.
        tracciati = []
        for cartella, sottocartelle, file in os.walk(RADICE):
            sottocartelle[:] = [s for s in sottocartelle if s != '.git']
            for nome in sorted(file):
                if nome in ('action.yml', 'action.yaml'):
                    tracciati.append(
                        os.path.relpath(os.path.join(cartella, nome), RADICE))
    for relativo in tracciati:
        if os.path.basename(relativo) in ('action.yml', 'action.yaml'):
            trovati.append(os.path.join(RADICE, relativo.replace('/', os.sep)))
    return sorted(set(trovati))


def autoverifica():
    """Il rilevatore deve vedere le forme mobili e non inventarne."""
    rifiutare = [
        '      - uses: actions/checkout@v4',
        '        uses: dtolnay/rust-toolchain@1.92.0',
        '        uses: actions/cache@main',
        # SHA abbreviata: ambigua, non e' un pin.
        '        uses: actions/checkout@11d5960 # v4',
        # SHA completa ma senza dire a quale versione corrisponde.
        '        uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262',
        # Chiave quotata, apici singoli, spazio prima dei due punti: forme
        # YAML valide che una regex sulla sola `uses:` non vede.
        '      - "uses": actions/checkout@v4',
        "      - 'uses': actions/checkout@v4",
        '        uses : actions/checkout@v4',
        # Valore quotato: gli apici non nascondono un tag mobile.
        '        uses: "actions/checkout@v4"',
        # Mapping in stile flow: non analizzabile riga per riga, quindi
        # rifiutata invece che ignorata.
        '      - { uses: actions/checkout@v4 }',
        # Immagine Docker con tag mobile: il tag si sposta come un ramo.
        '        uses: docker://alpine:3.20',
        '        uses: docker://alpine',
        # Chiave `uses` senza valore.
        '        uses:',
        # Flow mapping con una graffa DENTRO una stringa: la ricerca non
        # deve fermarsi li'.
        '      - { name: "}", uses: actions/checkout@v4 }',
        '      - { name: "] }", uses: actions/checkout@v4 }',
        # Chiave con escape YAML: e' la stessa chiave scritta in un'altra
        # forma, e non la si decodifica — la si rifiuta.
        '        "u\\u0073es": actions/checkout@v4',
        # Collezione ANNIDATA prima della chiave: una regex che si ferma
        # alla prima graffa chiusa non arriva a `uses`.
        '      - { env: { X: y }, uses: actions/checkout@v4 }',
        '      - [ { uses: actions/checkout@v4 } ]',
        # Chiave esplicita YAML: forma valida che questo gate non analizza.
        '      ? uses',
        '        - ? uses',
        # Chiave QUOTATA dentro una flow mapping: la parola sparisce se la
        # si cerca sul testo con le stringhe neutralizzate.
        '      - { "uses": actions/checkout@v4 }',
        "      - { 'uses': actions/checkout@v4 }",
        '      - { name: x, "uses": actions/checkout@v4 }',
        # Anchor e alias: un alias puo' fare da chiave e rendere `uses` una
        # chiave che il confronto letterale non vede.
        '        name: &use_key uses',
        '      - *use_key: actions/checkout@v4',
        '      - { *use_key: actions/checkout@v4 }',
        # Nomi di anchor che NON sono identificatori: YAML li ammette.
        '        name: &1 uses',
        '      - *1: actions/checkout@v4',
        '        name: &-x uses',
        '      - *-x: actions/checkout@v4',
        # Tag YAML sulla chiave: il parser di Actions legge il valore
        # risolto, il confronto testuale no.
        '      - !!str uses: actions/checkout@v4',
        '      - !custom uses: actions/checkout@v4',
        '        !!str uses: actions/checkout@v4',
        # `uses` che apre uno scalare a blocco: il riferimento sta nel
        # contenuto, e questo gate non lo risolve.
        '      - uses: >-\n          actions/checkout@v4\n',
        '      - uses: |\n          actions/checkout@v4\n',
        '        "uses": >-\n          actions/checkout@v4\n',
        # Chiave con ESCAPE che apre un blocco: risolve a `uses`, e il ramo
        # del blocco la salterebbe insieme a tutto il contenuto.
        '      - "u\\u0073es": >-\n          actions/checkout@v4\n',
        '      - "u\\u0073es": actions/checkout@v4',
        # Scalare a blocco aperto su una VOCE DI SEQUENZA: le chiavi sorelle
        # stanno alla colonna della chiave, non a quella del trattino, e
        # misurando dal trattino la `uses` sorella sparisce dentro il blocco.
        'steps:\n  - name: |\n      checkout\n    uses: actions/checkout@v4\n',
    ]
    for riga in rifiutare:
        if not problemi_nel_testo('sintetico', riga):
            raise SystemExit('autoverifica fallita: forma mobile non vista: %r' % riga)
    accettare = [
        '      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4',
        '        "uses": actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4',
        '        uses: ./.github/actions/locale',
        # Immagine Docker con digest immutabile.
        '        uses: docker://alpine@sha256:'
        + '0' * 64,
        # Non e' una dichiarazione `uses`: e' il valore di `run`.
        '        run: echo "uses: actions/checkout@v4"',
        '        run: grep -n "uses:" .github/workflows/ci.yml',
        # Un COMMENTO non e' YAML: puo' contenere enfasi con asterischi,
        # graffe e la parola `uses` senza che nulla di tutto cio' sia una
        # dichiarazione.
        '      # capire *dove* la coverage e\' scesa: usa & alias { }',
        '      # uses: actions/checkout@v4 (esempio nel commento)',
        # `&&` di shell dentro un comando: non e' un anchor, sta in mezzo a
        # un valore e non all'inizio di un nodo.
        '        run: cargo build && cargo test',
        '        run: apt-get update && apt-get install -y cmake',
        # Punto esclamativo dentro un comando: non e' un tag YAML.
        '        run: if ! test -f Cargo.lock; then exit 1; fi',
        '        run: test "$(git rev-parse HEAD)" != "$OLD" || exit 1',
        # Scalare a BLOCCO: dentro c'e' uno script, non YAML. Un glob di
        # shell non e' un tag e un `uses:` in un comando non e' una
        # dichiarazione.
        '        run: |\n'
        '          case "$M" in\n'
        '            0|*[!0-9]*) exit 2 ;;\n'
        '          esac\n'
        '          grep -n "uses:" ci.yml\n'
        '          echo "a" && echo "b"\n',
    ]
    # Dopo la chiusura del blocco la struttura torna YAML: una dichiarazione
    # mobile che segue uno script dev'essere vista.
    dopo_blocco = (
        '      - name: passo\n'
        '        run: |\n'
        '          echo "uses: actions/checkout@v4"\n'
        '      - uses: actions/checkout@v4\n'
    )
    if not problemi_nel_testo('sintetico', dopo_blocco):
        raise SystemExit(
            'autoverifica fallita: dichiarazione mobile dopo uno scalare a '
            'blocco non vista')
    for riga in accettare:
        if problemi_nel_testo('sintetico', riga):
            raise SystemExit('autoverifica fallita: falso positivo su: %r' % riga)
    return len(rifiutare), len(accettare)


def main():
    rifiutate, accettate = autoverifica()
    if not os.path.isdir(WORKFLOW):
        raise SystemExit('cartella dei workflow assente: %s' % WORKFLOW)
    problemi = []
    file = sorgenti_da_esaminare()
    conteggio = 0
    for percorso in file:
        nome = os.path.relpath(percorso, RADICE).replace(os.sep, '/')
        testo = io.open(percorso, encoding='utf-8', newline='').read()
        conteggio += len(voci_uses(testo))
        problemi.extend(problemi_nel_testo(nome, testo))
    if problemi:
        sys.stderr.write(
            'action non pinnate a una revisione esatta (docs/release.md '
            'dichiara la CI riproducibile):\n\n')
        for nome, numero, motivo in problemi:
            sys.stderr.write('- %s:%d: %s\n' % (nome, numero, motivo))
        sys.stderr.write(
            '\nUsare la SHA completa con un commento che ne dica la versione:\n'
            '  uses: owner/action@<40 esadecimali> # v4\n')
        raise SystemExit(1)
    print('action dei workflow tutte pinnate: %d file, %d riferimenti, '
          '%d forme mobili iniettate e viste, %d forme legittime lasciate '
          'passare' % (len(file), conteggio, rifiutate, accettate))


main()
