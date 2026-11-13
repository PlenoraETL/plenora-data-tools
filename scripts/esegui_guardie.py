"""Gate offline unico: raccoglie ogni esito e fallisce alla fine."""

from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def comandi():
    # Ogni guardia resta un eseguibile autonomo; la CI richiama questo runner.
    return [
        ["scripts/check_prove_fonti.py"],
        ["scripts/check_docs.py"],
        ["scripts/check_arrow_ingressi.py"],
        ["scripts/check_comments.py"],
        ["scripts/check_esito_unico.py"],
        ["scripts/genera_inventario.py", "--check"],
        ["scripts/genera_costi_operazioni.py", "--verifica"],
        ["scripts/genera_costi_geo.py", "--verifica"],
        ["-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"],
    ]


def esegui(commands, run=subprocess.run):
    failures = []
    for command in commands:
        print("GATE: " + " ".join(command), flush=True)
        try:
            result = run([sys.executable, *command], cwd=ROOT, check=False)
            code = result.returncode
        except OSError:
            code = -1
        if code:
            failures.append(command)
    for command in failures:
        print("FALLITA: " + " ".join(command))
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(esegui(comandi()))
