"""Regressioni sui token Rust: spazi, alias, delimitatori e confini test."""

import unittest

from rust_esito_tokens import external_test_modules, matches_findings, tokenize, unprotected_test_entries, has_method_exemption


class RustTokensTests(unittest.TestCase):
    def test_riattivazione_lint(self):
        self.assertEqual(len(unprotected_test_entries('#[cfg(test)] pub mod t;')), 1)
        self.assertEqual(len(unprotected_test_entries('#[test] fn t() {}')), 1)
        self.assertEqual(len(unprotected_test_entries('#[cfg(test)] const fn f() {}')), 1)
        self.assertEqual(unprotected_test_entries('#[cfg(test)] use super::*;'), [])
        self.assertEqual(unprotected_test_entries(
            '#[deny(clippy::disallowed_methods)] #[cfg(test)] mod t { #[test] fn f() {} }'), [])

    def test_pattern_raw_self_e_virgole(self):
        for source in [
            'matches!(f(), Err(_,))',
            'use std::matches::{self as m}; m!(f(), Err(_))',
            'matches!(f(), Some(r#if) | Err(_))',
        ]:
            self.assertIn('rifiuto_jolly', [r for _, r in matches_findings(source, True)])
        self.assertEqual(matches_findings('matches!(f(), | Err(E::A))', True), [])
        for visibility in ['pub', 'pub(crate)', 'pub(super)']:
            self.assertEqual(external_test_modules(f'#[cfg(test)] {visibility} mod t;', False), [('t', None, [])])

    def test_delimitatori_e_alias(self):
        for prefix, name in [('', 'matches'), ('use std::matches as m;', 'm'),
                             ('use core::{matches as m};', 'm'),
                             ('use std::{io, matches as m};', 'm'),
                             ('use {core::{matches as m}};', 'm')]:
            for left, right in [('(', ')'), ('[', ']'), ('{', '}')]:
                with self.subTest(prefix=prefix, left=left):
                    source = f'{prefix}\n{name} \n ! {left} f(), Err(_) {right}'
                    self.assertEqual([r for _, r in matches_findings(source, True)], ['rifiuto_jolly'])
                    precise = source.replace('Err(_)', 'Err(E::A)')
                    self.assertEqual(matches_findings(precise, True), [])

    def test_alternative_solo_nel_pattern(self):
        for pattern in ['Err(A) | Err(B)', 'Err(A | B)', 'Err(A) | Ok(_)']:
            self.assertIn('rifiuti_alternativi', [r for _, r in matches_findings(f'matches!(f(), {pattern})', True)])
        self.assertEqual(matches_findings('matches!(f(a | b), Err(E::A) if a || b)', True), [])

    def test_letterali_e_commenti(self):
        source = '''// matches!(f(), Err(_))
        /* nested /* matches ! {f(), Err(_)} */ */
        let a = r###"matches![f(), Err(_)]"###;
        let b = "matches!(f(), Err(_))";
        let c = '\"'; let d = b'!';
        fn f<'a>(x: &'a str) {}'''
        self.assertEqual(matches_findings(source, True), [])

    def test_produzione_e_test(self):
        source = 'fn f() { matches!(f(), Err(_)); } #[cfg(test)] mod t { fn h() { matches!{f(), Err(_)}; } }'
        self.assertEqual(len(matches_findings(source, False)), 1)
        self.assertEqual(matches_findings('fn f() { assert!(matches!(f(), Err(_))); }', False), [])

    def test_moduli_esterni(self):
        source = '#[cfg(test)] #[path = "other.rs"] mod t; mod production; #[cfg(test)] mod inner { mod helper; }'
        self.assertEqual(external_test_modules(source, False), [('t', 'other.rs', []), ('helper', None, ['inner'])])

    def test_input_incompleto_rifiutato(self):
        for source in ['/* incomplete', 'r###"incomplete', 'matches!(f(), Err(_)']:
            with self.assertRaises(ValueError):
                matches_findings(source, True)
        self.assertEqual([t.value for t in tokenize('r#matches ! (x, Err(_))')][:2], ['matches', '!'])

    def test_generici_nell_espressione(self):
        for expr in ['f::<u8, u16>(if true { 1 } else { 2 })', '<T as Trait<A, B>>::f()', 'f::<fn(u8, u8) -> u8, u16>()']:
            self.assertEqual([r for _, r in matches_findings(f'matches!({expr}, Err(_))', True)], ['rifiuto_jolly'])

    def test_generici_nel_costruttore_del_pattern(self):
        for constructor in ['Err::<(), ()>', 'Err::<fn() -> (), E>', 'Err::<Vec<Vec<u8>>, E>']:
            with self.subTest(constructor=constructor):
                self.assertEqual([r for _, r in matches_findings(
                    f'matches!(r, {constructor}(_))', True)], ['rifiuto_jolly'])
                self.assertEqual(matches_findings(
                    f'matches!(r, {constructor}(E::A))', True), [])
                self.assertIn('rifiuti_alternativi', [r for _, r in matches_findings(
                    f'matches!(r, {constructor}(E::A | E::B))', True)])

    def test_esenzione_senza_dipendere_dalla_motivazione(self):
        self.assertTrue(has_method_exemption('#![allow(clippy::disallowed_methods, reason="produzione")]'))
        self.assertFalse(has_method_exemption('// #![allow(clippy::disallowed_methods)]'))
        self.assertFalse(has_method_exemption('const S: &str = "Produzione: divieti degli esiti";'))
