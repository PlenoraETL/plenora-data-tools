"""Il gate locale esegue le prove esterne e rifiuta ogni loro fallimento."""

import contextlib
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import verifica_adozione


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
