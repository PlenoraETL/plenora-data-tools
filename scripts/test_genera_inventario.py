"""Prove del generatore di `docs/inventario.md` (`scripts/genera_inventario.py`).

Uso: python -m unittest discover -s scripts -p "test_*.py"
"""

from __future__ import annotations

import unittest

import genera_inventario


class ParsingTests(unittest.TestCase):
    def test_top_level_arguments_ignore_nested_commas_and_strings(self) -> None:
        self.assertEqual(
            genera_inventario.argomenti('"a,b", Some(X::Y), &[1, 2], f(g(h, i))'),
            ['"a,b"', "Some(X::Y)", "&[1, 2]", "f(g(h, i))"],
        )

    def test_brackets_in_strings_do_not_close(self) -> None:
        corpo, fine = genera_inventario.corpo_tra('x("a)b", c) resto', 1, "(", ")")
        self.assertEqual(corpo, '"a)b", c')
        self.assertEqual(fine, 10)

    def test_an_unclosed_bracket_is_an_error(self) -> None:
        with self.assertRaises(genera_inventario.ErroreInventario):
            genera_inventario.corpo_tra("x(a, b", 1, "(", ")")


class RepositoryTests(unittest.TestCase):
    def test_the_catalog_is_read_completely(self) -> None:
        voci = genera_inventario.voci_catalogo()
        identificatori = [voce["id"] for voce in voci]
        self.assertEqual(len(identificatori), len(set(identificatori)))
        self.assertIn("table.filter", identificatori)
        self.assertTrue(all(voce["famiglia"] in {"Table", "Geo"} for voce in voci))

    def test_render_is_deterministic_and_checked_in(self) -> None:
        primo = genera_inventario.render()
        self.assertEqual(primo, genera_inventario.render())
        self.assertEqual(genera_inventario.main(["--check"]), 0)


if __name__ == "__main__":
    unittest.main()
