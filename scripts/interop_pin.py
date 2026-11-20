"""Risolve una volta la release IO corrente in tag e commit immutabile."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.request import Request, urlopen

import strict_json

REPOSITORY = "PlenoraETL/plenora-IO-tools"


def risolvi(release, refs):
    if release.get("draft") is not False or release.get("prerelease") is not False:
        raise ValueError("release IO non stabile")
    tag = release.get("tag_name", "")
    if not re.fullmatch(r"v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag):
        raise ValueError("tag IO non valido")
    ref = "refs/tags/" + tag
    found = {}
    for line in refs.splitlines():
        fields = line.split()
        if len(fields) != 2 or not re.fullmatch(r"[0-9a-f]{40}", fields[0]):
            raise ValueError("riferimento IO non valido")
        sha, name = fields
        if name not in (ref, ref + "^{}") or name in found:
            raise ValueError("riferimento IO ambiguo")
        found[name] = sha
    if ref not in found:
        raise ValueError("tag IO assente")
    return {"repository": REPOSITORY, "tag": tag,
            "revision": found.get(ref + "^{}", found[ref]),
            "version": tag.removeprefix("v"), "release_id": release["id"]}


def corrente():
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "data-interop"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    request = Request(f"https://api.github.com/repos/{REPOSITORY}/releases/latest", headers=headers)
    with urlopen(request, timeout=60) as response:
        release = strict_json.loads(response.read())
    # Validare il tag prima di usarlo come argomento git; nessun checkout mobile.
    tag = release.get("tag_name", "")
    risolvi(release, "0" * 40 + "\trefs/tags/" + tag)
    refs = subprocess.run(
        ["git", "ls-remote", "https://github.com/" + REPOSITORY + ".git",
         "refs/tags/" + tag, "refs/tags/" + tag + "^{}"],
        capture_output=True, text=True, check=True, timeout=60,
    ).stdout
    return risolvi(release, refs)



def contesti_workflow(document):
    """Contesti per chiave dalla tabella GitHub Context availability.

    Controllo statico delle chiavi, non un interprete completo di espressioni.
    Chiavi non censite con espressioni vengono rifiutate.
    """
    base = "github needs strategy matrix inputs vars"
    runtime = base + " job runner env steps"
    rules = {
        "run-name": "github inputs vars", "concurrency": "github inputs vars",
        "env": "github secrets inputs vars",
        "jobs.*.env": base + " secrets",
        "jobs.*.if": "github needs vars inputs",
        "jobs.*.strategy": "github needs vars inputs",
        "jobs.*.defaults.run": base + " env",
        "jobs.*.outputs.*": runtime + " secrets",
        "jobs.*.secrets.*": base + " secrets",
        "jobs.*.with.*": base,
        "jobs.*.environment.url": runtime,
        "on.workflow_call.inputs.*.default": "github inputs vars",
        "on.workflow_call.outputs.*.value": "github jobs vars inputs",
    }
    for key in ("concurrency", "container", "container.image", "continue-on-error",
                "environment", "name", "runs-on", "services", "timeout-minutes"):
        rules["jobs.*." + key] = base
    for prefix in ("container", "services.*"):
        rules["jobs.*." + prefix + ".credentials"] = base + " env secrets"
        rules["jobs.*." + prefix + ".env.*"] = base + " job runner env secrets"
    for key in ("continue-on-error", "env", "name", "run", "timeout-minutes", "with", "working-directory"):
        rules["jobs.*.steps.*." + key] = runtime + " secrets"
    rules["jobs.*.steps.*.if"] = runtime
    patterns = [(k.split("."), set(v.split())) for k, v in rules.items()]
    errors = []

    def walk(value, path):
        if isinstance(value, dict):
            for key, child in value.items():
                if "${{" in str(key):
                    errors.append("chiave dinamica non ammessa")
                walk(child, path + [str(key)])
        elif isinstance(value, list):
            for index, child in enumerate(value):
                walk(child, path + [str(index)])
        elif isinstance(value, str):
            expressions = re.findall(r"\$\{\{(.*?)\}\}", value, re.S)
            if "${{" in re.sub(r"\$\{\{.*?\}\}", "", value, flags=re.S):
                errors.append("espressione non terminata")
            if path[-1:] == ["if"] and not expressions:
                expressions = [value]
            if not expressions:
                return
            matches = [(len(parts), allowed) for parts, allowed in patterns
                       if len(parts) <= len(path) and
                       all(a == "*" or a == b for a, b in zip(parts, path))]
            if not matches:
                errors.append("espressione in chiave non ammessa: " + ".".join(path))
            allowed = max(matches, key=lambda item: item[0])[1] if matches else set()
            for expression in expressions:
                # Stringhe letterali non sono riferimenti a contesti.
                expression = re.sub(r"'(?:[^']|'')*'", "''", expression)
                for match in re.finditer(r"(?<![\w.])[A-Za-z_][\w-]*", expression):
                    name = match[0]
                    tail = expression[match.end():].lstrip()
                    if tail.startswith("("):
                        special = name.lower()
                        step = len(path) >= 5 and path[2] == "steps"
                        if special == "hashfiles" and not step:
                            errors.append("hashFiles non ammesso: " + ".".join(path))
                        if special in {"always", "cancelled", "success", "failure"} and path[-1] != "if":
                            errors.append("funzione di stato non ammessa: " + ".".join(path))
                    elif name.lower() not in {"true", "false", "null"} and name not in allowed:
                        errors.append("contesto non ammesso: " + ".".join(path) + ": " + name)
    walk(document, [])
    return errors


def verifica_workflow():
    import yaml

    class UniqueLoader(yaml.BaseLoader):
        pass

    def mapping(loader, node):
        result = {}
        for key_node, value_node in node.value:
            key = loader.construct_object(key_node)
            if key in result:
                raise ValueError("chiave YAML duplicata")
            result[key] = loader.construct_object(value_node)
        return result

    UniqueLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, mapping)
    root = Path(__file__).resolve().parents[1]
    paths = sorted((root / ".github/workflows").glob("*.y*ml"))
    for path in paths:
        document = yaml.load(path.read_text("utf-8"), Loader=UniqueLoader)
        errors = contesti_workflow(document)
        if errors:
            raise ValueError(path.name + ": " + "; ".join(errors))
        print(path.name + ": YAML e contesti validi")
    return len(paths)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--check-workflows", action="store_true")
    args = parser.parse_args()
    if args.check_workflows:
        verifica_workflow()
        return
    if args.report is None:
        parser.error("--report richiesto per risolvere la release")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    try:
        pin = corrente()
    except Exception as error:
        args.report.write_text(json.dumps({"stato": "fallito", "errore": type(error).__name__}) + "\n", encoding="utf-8")
        raise SystemExit("risoluzione release IO fallita") from None
    args.report.write_text(json.dumps(pin, indent=2) + "\n", encoding="utf-8")
    print("revision=" + pin["revision"])
    print("tag=" + pin["tag"])


if __name__ == "__main__":
    main()
