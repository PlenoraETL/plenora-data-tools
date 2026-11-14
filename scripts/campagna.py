"""Raccolta completa di casi indipendenti, con errori senza dati."""


def raccogli(cases, execute):
    results = []
    for case in cases:
        try:
            evidence = execute(case)
        except Exception as error:
            # Il messaggio del processo o dell'assertion potrebbe contenere dati.
            results.append({"caso": case, "stato": "fallito", "errore": type(error).__name__})
        else:
            results.append({"caso": case, "stato": "superato", "evidenza": evidence})
    if not results:
        raise ValueError("campagna senza casi")
    return results


def fallita(results):
    return not results or any(r["stato"] != "superato" for r in results)
