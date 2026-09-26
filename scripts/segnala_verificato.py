#!/usr/bin/env python3
"""Manda un segnale a un processo **solo se e' ancora quello atteso**.

Uso::

    segnala_verificato.py PID SEGNALE --eseguibile PERCORSO [--genitore PPID]
                          [--argomento TESTO]

L'eseguibile e' obbligatorio; genitore e argomento si aggiungono, e quelli
dati devono tornare tutti.

Un PID letto o ricordato da uno script di shell puo' tornare in uso prima del
segnale: Bash miete i figli in modo asincrono, nel gestore di SIGCHLD, quindi
nemmeno un figlio «non ancora atteso» tiene occupato il proprio PID. Qui il
processo si apre con un ``pidfd``, se ne verifica l'identita' leggendo
``/proc/PID`` e il segnale parte **attraverso il pidfd**. Se il PID e' gia'
stato riusato quando il pidfd si apre, l'identita' non torna e non parte
niente; se il processo muore dopo l'apertura, il pidfd non puo' raggiungere il
suo successore e il segnale non arriva a nessuno.

L'identita' e' l'eseguibile (``/proc/PID/exe``), un argomento esatto della
riga di comando, e il genitore (``PPid``) quando chi chiama lo conosce: un
processo riparentato non ha un genitore prevedibile. Un argomento da solo non
basta mai: questo stesso processo lo porta nei propri argomenti, e con lo
stesso genitore potrebbe riconoscersi nel bersaglio se ne ereditasse il PID.
L'eseguibile lo distingue, perche' qui e' l'interprete Python. Sceglierli e'
compito di chi chiama.

Uscita: 0 segnale consegnato; 1 processo assente o diverso, nessun segnale;
2 uso sbagliato o piattaforma senza pidfd.
"""

import argparse
import os
import signal
import sys


def identita_torna(pid, genitore, eseguibile, argomento):
    """Se ``/proc/PID`` descrive il processo atteso."""
    try:
        with open(f"/proc/{pid}/status", encoding="utf-8") as stato:
            ppid = next(
                (riga.split()[1] for riga in stato if riga.startswith("PPid:")), None
            )
        if genitore is not None and ppid != str(genitore):
            return False
        if eseguibile is not None and os.readlink(f"/proc/{pid}/exe") != os.path.realpath(
            eseguibile
        ):
            return False
        if argomento is not None:
            with open(f"/proc/{pid}/cmdline", "rb") as riga:
                argomenti = riga.read().split(b"\0")
            if argomento.encode() not in argomenti:
                return False
        return True
    except OSError:
        return False


def segnala(pid, numero, genitore=None, eseguibile=None, argomento=None):
    """Rende 0 se il segnale e' partito verso il processo atteso, 1 altrimenti."""
    if eseguibile is None:
        return 1
    try:
        descrittore = os.pidfd_open(pid)
    except ProcessLookupError:
        return 1
    try:
        if not identita_torna(pid, genitore, eseguibile, argomento):
            return 1
        try:
            signal.pidfd_send_signal(descrittore, numero)
        except ProcessLookupError:
            return 1
        return 0
    finally:
        os.close(descrittore)


def main(argomenti):
    lettore = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    lettore.add_argument("pid", type=int)
    lettore.add_argument("segnale", help="nome senza SIG, per esempio INT o KILL")
    lettore.add_argument("--genitore", type=int)
    lettore.add_argument("--eseguibile", required=True)
    lettore.add_argument("--argomento")
    opzioni = lettore.parse_args(argomenti)
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        print("segnala_verificato: questa piattaforma non ha pidfd", file=sys.stderr)
        return 2
    try:
        numero = signal.Signals[f"SIG{opzioni.segnale}"]
    except KeyError:
        print(f"segnala_verificato: segnale sconosciuto {opzioni.segnale}", file=sys.stderr)
        return 2
    return segnala(
        opzioni.pid, numero, opzioni.genitore, opzioni.eseguibile, opzioni.argomento
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
