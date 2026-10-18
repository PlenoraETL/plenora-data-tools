"""Il gate locale esegue le prove esterne e rifiuta ogni loro fallimento."""

import contextlib
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import genera_manifesto_adozione
import verifica_adozione


class VerificaCheckoutTests(unittest.TestCase):
    """Le prove del pin eseguono il worktree, i digest leggono i blob.

    HEAD giusto non basta: un file modificato o aggiunto nel checkout farebbe
    eseguire al gate prove diverse da quelle adottate, con i digest ancora
    verdi perche' letti da Git.
    """

    def esiti(self, stato: str, voci: str = "H tools/validate_specs.py\n"):
        return [
            subprocess.CompletedProcess([], 0, "abc\n", ""),
            subprocess.CompletedProcess([], 0, stato, ""),
            subprocess.CompletedProcess([], 0, voci, ""),
        ]

    def test_un_worktree_sporco_ferma_il_gate(self):
        for stato in (
            " M tools/validate_specs.py\n",  # tracciato modificato
            "?? tools/ombra.py\n",  # aggiunto
            "!! tools/__pycache__/\n",  # ignorato: Python legge i .pyc anche con -B
        ):
            with self.subTest(stato=stato):
                with patch.object(
                    genera_manifesto_adozione.subprocess, "run",
                    side_effect=self.esiti(stato),
                ):
                    with self.assertRaises(SystemExit):
                        genera_manifesto_adozione.verifica_checkout(Path("contratti"), "abc")

    def test_i_bit_dell_indice_non_nascondono_modifiche(self):
        for voci in ("h tools/validate_specs.py\n", "S tools/messages.py\n"):
            with self.subTest(voci=voci):
                with patch.object(
                    genera_manifesto_adozione.subprocess, "run",
                    side_effect=self.esiti("", voci),
                ):
                    with self.assertRaises(SystemExit):
                        genera_manifesto_adozione.verifica_checkout(Path("contratti"), "abc")

    def test_un_worktree_pulito_passa(self):
        with patch.object(
            genera_manifesto_adozione.subprocess, "run", side_effect=self.esiti("\n"),
        ):
            genera_manifesto_adozione.verifica_checkout(Path("contratti"), "abc")

    def test_lo_stato_si_chiede_a_git_senza_sconti(self):
        """Le configurazioni locali non devono poter nascondere nulla.

        `--untracked-files=all` vince su `status.showUntrackedFiles=no` e
        `--ignored=matching` mostra anche cio' che `.gitignore` o
        `core.excludesFile` escluderebbero: senza i due flag, proprio il
        modulo aggiunto che il controllo cerca resterebbe invisibile.
        """
        with patch.object(
            genera_manifesto_adozione.subprocess, "run", side_effect=self.esiti(""),
        ) as run:
            genera_manifesto_adozione.verifica_checkout(Path("contratti"), "abc")
        comando = run.call_args_list[1].args[0]
        self.assertIn("--untracked-files=all", comando)
        self.assertIn("--ignored=matching", comando)
        self.assertIn("ls-files", run.call_args_list[2].args[0])


class ProveContrattiTests(unittest.TestCase):
    def test_esegue_tutte_le_prove_dichiarate(self):
        with patch.object(verifica_adozione.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            with contextlib.redirect_stdout(io.StringIO()):
                verifica_adozione.prove_contratti(Path("contratti"))
        self.assertEqual(
            [Path(call.args[0][-1]).name for call in run.call_args_list],
            ["validate_specs.py", "test_messages.py", "test_gate_fuzz.py",
             "test_message_sentinels.py"],
        )

    def test_ogni_fallimento_ferma_il_gate_senza_dati(self):
        for posizione in range(4):
            with self.subTest(posizione=posizione):
                risultati = [subprocess.CompletedProcess([], 0)] * posizione
                risultati.append(subprocess.CompletedProcess([], 1, b"SEGRETO", b"SEGRETO"))
                output = io.StringIO()
                with patch.object(verifica_adozione.subprocess, "run", side_effect=risultati) as run:
                    with contextlib.redirect_stdout(output):
                        with self.assertRaisesRegex(ValueError, "^prova dei contratti fallita$"):
                            verifica_adozione.prove_contratti(Path("contratti"))
                self.assertEqual(run.call_count, posizione + 1)
                self.assertNotIn("SEGRETO", output.getvalue())


if __name__ == "__main__":
    unittest.main()
