"""Casi di ``segnala_verificato``: su Linux, con processi veri."""

import os
import signal
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))

import segnala_verificato  # noqa: E402

HA_PIDFD = hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal")


@unittest.skipUnless(HA_PIDFD, "serve pidfd (Linux, Python 3.9+)")
class SegnalaVerificato(unittest.TestCase):
    def figlio(self):
        processo = subprocess.Popen(["sleep", "30"])
        self.addCleanup(lambda: (processo.kill(), processo.wait()))
        # L'identita' si legge da /proc: il caso comincia quando il figlio e'
        # gia' `sleep`, non mentre l'exec e' ancora in corso. Un'identita' non
        # ancora visibile fa rifiutare il segnale, ed e' il verso prudente.
        scadenza = time.monotonic() + 5
        while time.monotonic() < scadenza:
            with open(f"/proc/{processo.pid}/cmdline", "rb") as riga:
                if riga.read().split(b"\0")[:2] == [b"sleep", b"30"]:
                    return processo
            time.sleep(0.01)
        self.fail("il figlio non e' diventato `sleep 30` entro 5 s")

    def test_l_identita_giusta_riceve_il_segnale(self):
        processo = self.figlio()
        esito = segnala_verificato.segnala(
            processo.pid, signal.SIGTERM, os.getpid(), argomento="30"
        )
        self.assertEqual(esito, 0)
        self.assertEqual(processo.wait(timeout=5), -signal.SIGTERM)

    def test_un_genitore_diverso_non_riceve_niente(self):
        processo = self.figlio()
        esito = segnala_verificato.segnala(
            processo.pid, signal.SIGTERM, os.getpid() + 1, argomento="30"
        )
        self.assertEqual(esito, 1)
        self.assertIsNone(processo.poll())

    def test_un_eseguibile_diverso_non_riceve_niente(self):
        processo = self.figlio()
        esito = segnala_verificato.segnala(
            processo.pid, signal.SIGTERM, os.getpid(), eseguibile=sys.executable
        )
        self.assertEqual(esito, 1)
        self.assertIsNone(processo.poll())

    def test_un_processo_mietuto_non_riceve_niente(self):
        processo = subprocess.Popen(["true"])
        processo.wait()
        esito = segnala_verificato.segnala(
            processo.pid, signal.SIGTERM, os.getpid(), argomento="true"
        )
        self.assertEqual(esito, 1)


if __name__ == "__main__":
    unittest.main()
