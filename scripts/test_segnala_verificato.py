"""Casi di ``segnala_verificato``: su Linux, con processi veri."""

import os
import shutil
import signal
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))

import segnala_verificato  # noqa: E402

HA_PIDFD = hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal")


def processo_sleep():
    """Il percorso reale di `sleep`, come lo vede /proc/PID/exe."""
    return os.path.realpath(shutil.which("sleep"))


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
            processo.pid,
            signal.SIGTERM,
            os.getpid(),
            eseguibile=processo_sleep(),
            argomento="30",
        )
        self.assertEqual(esito, 0)
        self.assertEqual(processo.wait(timeout=5), -signal.SIGTERM)

    def test_un_genitore_diverso_non_riceve_niente(self):
        processo = self.figlio()
        esito = segnala_verificato.segnala(
            processo.pid, signal.SIGTERM, os.getpid() + 1, eseguibile=processo_sleep()
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

    def test_chi_segnala_non_si_riconosce_nel_bersaglio(self):
        # Stesso genitore e stesso argomento, ma l'eseguibile e' l'interprete:
        # l'aiuto non segnala se stesso anche se ne avesse ereditato il PID.
        esito = segnala_verificato.segnala(
            os.getpid(),
            signal.SIGTERM,
            os.getppid(),
            eseguibile="/usr/bin/sleep",
            argomento=sys.argv[0],
        )
        self.assertEqual(esito, 1)

    def test_eseguibile_e_argomento_devono_tornare_entrambi(self):
        processo = self.figlio()
        esito = segnala_verificato.segnala(
            processo.pid,
            signal.SIGTERM,
            os.getpid(),
            eseguibile=processo_sleep(),
            argomento="31",
        )
        self.assertEqual(esito, 1)
        self.assertIsNone(processo.poll())
        esito = segnala_verificato.segnala(
            processo.pid,
            signal.SIGTERM,
            os.getpid(),
            eseguibile=processo_sleep(),
            argomento="30",
        )
        self.assertEqual(esito, 0)
        self.assertEqual(processo.wait(timeout=5), -signal.SIGTERM)

    def test_senza_eseguibile_non_parte_niente(self):
        # L'argomento da solo non basta: lo porta anche chi segnala.
        processo = self.figlio()
        self.assertEqual(
            segnala_verificato.segnala(
                processo.pid, signal.SIGTERM, os.getpid(), argomento="30"
            ),
            1,
        )
        self.assertIsNone(processo.poll())

    def test_un_processo_mietuto_non_riceve_niente(self):
        processo = subprocess.Popen(["true"])
        processo.wait()
        esito = segnala_verificato.segnala(
            processo.pid,
            signal.SIGTERM,
            os.getpid(),
            eseguibile=os.path.realpath(shutil.which("true")),
        )
        self.assertEqual(esito, 1)


if __name__ == "__main__":
    unittest.main()
