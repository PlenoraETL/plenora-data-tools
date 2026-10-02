"""Prove statiche di `scripts/check_cargo_deny.py` (nessun Docker richiesto).

Uso: python -m unittest discover -s scripts -p "test_*.py"
"""

from __future__ import annotations

import tomllib
import unittest

import check_cargo_deny


class CargoDenyContainerTests(unittest.TestCase):
    def test_immagine_e_strumento_sono_fissati(self) -> None:
        dockerfile = check_cargo_deny.DOCKERFILE.read_text(encoding="utf-8")
        self.assertRegex(dockerfile.splitlines()[0], r"^FROM rust@sha256:[0-9a-f]{64}$")
        self.assertIn("CARGO_DENY_VERSION=0.20.2", dockerfile)
        self.assertIn("cargo install --locked", dockerfile)

    def test_repository_in_sola_lettura_e_container_effimero(self) -> None:
        _, workspace, fuzz = check_cargo_deny.commands()
        for comando in (workspace, fuzz):
            self.assertIn("--rm", comando)
            self.assertIn(f"{check_cargo_deny.ROOT}:/workspace:ro", comando)
        self.assertEqual(workspace[-2:], ["check", "--hide-inclusion-graph"])
        self.assertEqual(
            fuzz[-4:],
            ["--manifest-path", "fuzz/Cargo.toml", "check", "--hide-inclusion-graph"],
        )

    def test_solo_i_crate_privati_escono_dalla_verifica_delle_licenze(self) -> None:
        policy = tomllib.loads((check_cargo_deny.ROOT / "deny.toml").read_text(encoding="utf-8"))
        workspace = tomllib.loads((check_cargo_deny.ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.assertTrue(policy["licenses"]["private"]["ignore"])
        self.assertFalse(workspace["workspace"]["package"]["publish"])
        self.assertTrue(policy["licenses"]["allow"])

    def test_nessuna_advisory_ignorata(self) -> None:
        policy = tomllib.loads((check_cargo_deny.ROOT / "deny.toml").read_text(encoding="utf-8"))
        self.assertEqual(policy["advisories"]["ignore"], [])
        self.assertEqual(policy["advisories"]["yanked"], "deny")


if __name__ == "__main__":
    unittest.main()
