"""Riconcilia la migrazione heapless deliberata; non concede nuove eccezioni.

Uso: python .ci/aggiorna_dependency_policy.py (fuori CI).
Conserva baseline, altri repository e checker dello snapshot.
"""
import copy
import hashlib
import importlib.util
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
NAME = "plenora-data-tools2"
OLD_PIN = "09f47749e59fdf518e49386975b2f58fb8a61b9ade91da7db3136ab5f3bc78d0"
CHECKSUM = "411fdef4df93751117bd9ef8caa0e7f99c65dafec5bd84313eb8e2fdd75e32f2"
FORK_HASH = "985f64c3b6f7f6e59331e3b8b49f85d080257b7b1e850780bff237546a0983ae"
FORK_PATH = "vendor/rstar-0.12.2-heapless"


def carica_gate(home):
    spec = importlib.util.spec_from_file_location("dependency_policy", home / "controlla_dipendenze.py")
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)
    return gate


def blocchi(data):
    return re.split(rb"(?m)(?=^\[\[\w+\]\]\r?$)", data)


def rimuovi_orfane(data, policy, report):
    if any(e["rule"] != "stale_exception" or e["repo"] != NAME for e in report["errors"]):
        raise ValueError("deviazioni attive: riconciliazione rifiutata")
    stale = {e["key"] for e in report["errors"]}
    kept, removed = [], set()
    for part in blocchi(data):
        if part.startswith(b"[[exceptions]]"):
            entry = tomllib.loads(part.decode("utf8"))["exceptions"][0]
            if entry["repo"] == NAME and entry["key"] in stale:
                removed.add(entry["key"])
                continue
        kept.append(part)
    result = b"".join(kept)
    expected = copy.deepcopy(policy)
    expected["exceptions"] = [e for e in policy["exceptions"]
                              if not (e["repo"] == NAME and e["key"] in stale)]
    if removed != stale or tomllib.loads(result.decode("utf8")) != expected:
        raise ValueError("riconciliazione non conservativa")
    return result


def qualifica(data, root):
    # Identita approvate esplicitamente: non promuovere qualsiasi lock futuro.
    for lock in ("Cargo.lock", "fuzz/Cargo.lock"):
        packages = tomllib.loads((root / lock).read_text("utf8"))["package"]
        heapless = [p for p in packages if p["name"] == "heapless"]
        forks = [p for p in packages if p["name"] == "plenora-rstar"]
        if (len(heapless) != 1 or heapless[0]["version"] != "0.9.4"
                or heapless[0].get("checksum") != CHECKSUM
                or heapless[0].get("source") != "registry+https://github.com/rust-lang/crates.io-index"
                or len(forks) != 1 or forks[0]["version"] != "0.12.2"
                or "source" in forks[0] or any(p["name"] == "rstar" for p in packages)):
            raise ValueError("lock diverso dalla migrazione qualificata")
    if hashlib.sha256((root / FORK_PATH / "Cargo.toml").read_bytes()).hexdigest() != FORK_HASH:
        raise ValueError("fork diverso dalla migrazione qualificata")
    reference = f'''[[references]]
crate = "heapless"
line = "0.9"
version = "0.9.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{CHECKSUM}"
provenance = "Correzioni RUSTSEC-2026-0574..0582, 2026-11-20"

'''.encode()
    fork = f'''[[forks]]
repo = "{NAME}"
crate = "plenora-rstar"
version = "0.12.2"
source = "path:{FORK_PATH}"
manifest_sha256 = "{FORK_HASH}"
provenance = "{FORK_PATH}/PROVENANCE.md; fork di famiglia per usare heapless 0.9.4 corretto mantenendo API e algoritmi rstar 0.12.2"
removal = "quando rstar pubblica una versione con heapless corretta"

'''.encode()
    kept = []
    for part in blocchi(data):
        parsed = tomllib.loads(part.decode("utf8"))
        if part.startswith(b"[[references]]") and parsed["references"][0]["crate"] == "heapless" and parsed["references"][0]["line"] == "0.9":
            continue
        if part.startswith(b"[[forks]]") and parsed["forks"][0]["crate"] == "plenora-rstar" and parsed["forks"][0]["repo"] == NAME:
            continue
        kept.append(part)
    result = b"".join(kept).rstrip() + b"\n\n" + reference + fork
    result, count = re.subn(rb'(?m)^"heapless" = "0\.[89]"\r?$', b'"heapless" = "0.9"', result)
    if count != 1:
        raise ValueError("linea heapless assente o inattesa")
    return result.rstrip() + b"\n"


def aggiorna(root=ROOT):
    home = root / ".ci/plenora-dependencies"
    manifest = home / "plenora-dependencies.toml"
    digest = home / "plenora-dependencies.sha256"
    snapshot = home / "snapshot.sha256"
    gate = carica_gate(home)
    gate.load_policy(manifest, digest)
    entries = []
    for line in snapshot.read_text("ascii").splitlines():
        expected, filename = line.split()
        path = home / filename
        if path.parent != home or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError("snapshot iniziale non integro")
        entries.append(path)
    if set(entries) != {manifest, digest, home / "controlla_dipendenze.py"} or len(entries) != 3:
        raise ValueError("snapshot incompleto o duplicato")
    data = qualifica(manifest.read_bytes(), root)
    policy = tomllib.loads(data.decode("utf8"))
    report = gate.evaluate(policy, {NAME: root})
    data = rimuovi_orfane(data, policy, report)
    if gate.evaluate(tomllib.loads(data.decode("utf8")), {NAME: root})["errors"]:
        raise ValueError("policy riconciliata non supera il gate")
    old_hash = hashlib.sha256(manifest.read_bytes()).hexdigest()
    new_hash = hashlib.sha256(data).hexdigest()
    workflow = root / ".github/workflows/ci.yml"
    workflow_data = workflow.read_bytes()
    pins = re.findall(rb"([0-9a-f]{64})  plenora-dependencies.toml", workflow_data)
    # Ammette il solo pin storico noto, oltre al digest corrente verificato.
    if len(pins) != 1 or pins[0].decode() not in (old_hash, OLD_PIN):
        raise ValueError("pin workflow assente, ambiguo o sconosciuto")
    writes = {manifest: data, digest: (new_hash + "  plenora-dependencies.toml\n").encode()}
    writes[snapshot] = "".join(hashlib.sha256(writes.get(p, p.read_bytes())).hexdigest()
                                + "  " + p.name + "\n" for p in entries).encode()
    writes[workflow] = workflow_data.replace(pins[0] + b"  plenora-dependencies.toml",
                                            new_hash.encode() + b"  plenora-dependencies.toml")
    for path, content in writes.items():
        if path.read_bytes() != content:
            path.write_bytes(content)
    print(f"Rimosse {len(report['errors'])} eccezioni orfane; qualifiche, snapshot e pin coerenti.")


if __name__ == "__main__":
    aggiorna()
