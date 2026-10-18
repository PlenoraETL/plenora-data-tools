"""Prove dei grafi, avvisi e artefatti: corruzioni e omissioni falliscono."""
from __future__ import annotations

import copy
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import avvisi_crate as a
import licenze_artefatto as l


class Avvisi(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.texts = self.root / 'testi'
        self.file = self.texts / 'demo-1.0/LICENSE'
        self.file.parent.mkdir(parents=True)
        self.file.write_bytes(b'Copyright Demo\nMIT License\n')
        self.data = {'schema_version': 1, 'grafi': {'cli/test': ['demo-1.0']}, 'crate': {
            'demo-1.0': {'licenza': 'MIT', 'avvisi': [
                {'file': 'LICENSE', 'sha256': a.impronta(self.file.read_bytes()), 'origine': 'pacchetto'}]}}}
        for name, value in [('TESTI', self.texts)]:
            ctx = patch.object(a, name, value)
            ctx.start()
            self.addCleanup(ctx.stop)
        ctx = patch.object(a, 'registro', return_value=self.data)
        ctx.start()
        self.addCleanup(ctx.stop)

    def test_integrita_e_fine_riga(self):
        expected = a.contenuti(self.data)
        self.file.write_bytes(self.file.read_bytes().replace(b'\n', b'\r\n'))
        self.assertEqual(a.contenuti(self.data), expected)
        self.file.write_bytes(b'alterato')
        with self.assertRaises(ValueError):
            a.contenuti(self.data)

    def test_mancanza_registrata_non_e_deroga(self):
        self.data['crate']['altro-1'] = {'avvisi': [], 'mancante': {'ragione': 'cache assente'}}
        self.data['grafi']['cli/test'].append('altro-1')
        with self.assertRaisesRegex(ValueError, 'rilascio bloccato'):
            a.contenuti(self.data)

    def test_voce_obsoleta(self):
        self.data['grafi']['cli/test'] = []
        with self.assertRaises(ValueError):
            a.contenuti(self.data)

    def test_soli_rinvii_rifiutati_salvo_eccezione_ancorata(self):
        """Il caso siphasher: un COPYING di soli rinvii non e un avviso completo.

        E il controcaso snap: un COPYING che e il testo pieno passa, ma solo
        per eccezione dichiarata e ancorata all'impronta di quei byte.
        """
        testo = b'Copyright X\nSee LICENSE-MIT\n'
        (self.texts / 'rinvio-1').mkdir()
        (self.texts / 'rinvio-1/COPYING').write_bytes(testo)
        self.data['grafi']['cli/test'].append('rinvio-1')
        self.data['crate']['rinvio-1'] = {'licenza': 'MIT', 'avvisi': [
            {'file': 'COPYING', 'sha256': a.impronta(testo), 'origine': 'pacchetto'}]}
        with self.assertRaisesRegex(ValueError, 'rinvii'):
            a.contenuti(self.data)
        with patch.object(a, 'COPYING_COMPLETO', {('rinvio-1', a.impronta(testo))}):
            a.contenuti(self.data)
        # L'eccezione muore coi byte: un testo diverso con la stessa voce
        # d'eccezione resta rifiutato.
        with patch.object(a, 'COPYING_COMPLETO', {('rinvio-1', 'deadbeef' * 8)}):
            with self.assertRaisesRegex(ValueError, 'rinvii'):
                a.contenuti(self.data)

    def test_supplementare_senza_provenienza_rifiutato(self):
        """Un testo non del pacchetto entra solo con origine https e blob git.

        Il blob si ricalcola offline dai byte conservati (forme LF o CRLF):
        origine vuota, blob assente o blob che non e quello del testo sono
        tre modi di dichiarare una provenienza che non c'e.
        """
        testo = b'Permission is hereby granted...\n'
        (self.file.parent / 'LICENSE-MIT').write_bytes(testo)
        voce = {'file': 'LICENSE-MIT', 'sha256': a.impronta(testo),
                'origine': 'https://example.invalid/LICENSE-MIT',
                'git_blob': a.impronte_blob(testo)[0]}
        self.data['crate']['demo-1.0']['avvisi'].append(voce)
        a.contenuti(self.data)
        for campo, valore in (('origine', ''), ('origine', 'git@example.invalid:x'),
                              ('git_blob', ''), ('git_blob', '0' * 40)):
            with self.subTest(campo=campo, valore=valore):
                rotto = dict(voce)
                rotto[campo] = valore
                self.data['crate']['demo-1.0']['avvisi'][-1] = rotto
                with self.assertRaisesRegex(ValueError, 'provenienza|blob git'):
                    a.contenuti(self.data)
        # Il CRLF di un checkout Windows non cambia il verdetto.
        self.data['crate']['demo-1.0']['avvisi'][-1] = voce
        (self.file.parent / 'LICENSE-MIT').write_bytes(testo.replace(b'\n', b'\r\n'))
        a.contenuti(self.data)

    def test_testo_estraneo(self):
        (self.file.parent / 'NOTICE').write_bytes(b'n')
        with self.assertRaises(ValueError):
            a.contenuti(self.data)

    def test_percorsi_e_duplicati(self):
        for name in ['../x', '/x', 'x\\y', 'x//y', 'C:/x']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                a.percorso(name)
        self.data['crate']['demo-1.0']['avvisi'] *= 2
        with self.assertRaises(ValueError):
            a.contenuti(self.data)

    def test_licenze_annidate_e_notice(self):
        (self.root / 'native').mkdir()
        (self.root / 'LICENSE').write_bytes(b'l')
        (self.root / 'native/NOTICE').write_bytes(b'n')
        files = a.file_del_pacchetto({'manifest_path': str(self.root / 'Cargo.toml')})
        self.assertIn('native/NOTICE', files)
        self.assertIn('LICENSE', files)
        with self.assertRaises(ValueError):
            a.file_del_pacchetto({'manifest_path': str(self.root / 'Cargo.toml'), 'license_file': '../missing'})

    def cli(self):
        binary = self.root / 'plenora-data'
        binary.write_bytes(b'ELF fixture')
        dest = self.root / 'cli.zip'
        l.confeziona(binary, dest, 'cli')
        return dest

    def test_cli_e_determinismo(self):
        dest = self.cli()
        first = dest.read_bytes()
        l.verifica(dest, 'cli')
        self.cli()
        self.assertEqual(dest.read_bytes(), first)
        with zipfile.ZipFile(dest) as z:
            self.assertEqual(z.getinfo('plenora-data').external_attr >> 16, 0o100755)

    def test_crate_intero_rimosso_anche_dal_manifesto(self):
        dest = self.cli()
        files = l.leggi(dest)
        del files['LICENSES/crate-rust/demo-1.0/LICENSE']
        forged = copy.deepcopy(self.data)
        forged['crate'] = {}
        files['LICENSES/crate-rust/CRATE.json'] = a.json_bytes(forged)
        with zipfile.ZipFile(dest, 'w') as z:
            for n, c in files.items():
                z.writestr(n, c)
        with self.assertRaises(ValueError):
            l.verifica(dest, 'cli')

    def test_wheel_record_e_modulo_conservato(self):
        dest = self.root / 'demo.whl'
        with zipfile.ZipFile(dest, 'w') as z:
            z.writestr('plenora_data/_native.pyd', b'native')
            z.writestr('plenora_data-1.dist-info/METADATA', b'Name: plenora-data\nVersion: 1\n')
            z.writestr('plenora_data-1.dist-info/RECORD', b'')
        l.confeziona(dest, dest, 'wheel')
        l.verifica(dest, 'wheel')
        self.assertEqual(l.leggi(dest)['plenora_data/_native.pyd'], b'native')
        files = l.leggi(dest)
        files['plenora_data/_native.pyd'] = b'changed'
        with zipfile.ZipFile(dest, 'w') as z:
            for n, c in files.items():
                z.writestr(n, c)
        with self.assertRaisesRegex(ValueError, 'RECORD'):
            l.verifica(dest, 'wheel')

    def test_sorgente_e_modo_eseguibile(self):
        path = self.root / 'source.tar.gz'
        with tarfile.open(path, 'w:gz') as tar:
            for name in ['source/Cargo.lock', 'source/tool.sh']:
                entry = tarfile.TarInfo(name)
                entry.mode = 0o755
                entry.size = 1
                tar.addfile(entry, io.BytesIO(b'x'))
        l.confeziona(path, path, 'sorgente')
        l.verifica(path, 'sorgente')
        with tarfile.open(path) as tar:
            self.assertEqual(tar.getmember('source/tool.sh').mode, 0o755)

    def test_wheel_firmato_non_si_modifica(self):
        path = self.root / 'signed.whl'
        with zipfile.ZipFile(path, 'w') as z:
            z.writestr('p.dist-info/METADATA', b'Name: p')
            z.writestr('p.dist-info/RECORD.jws', b'signature')
        original = path.read_bytes()
        with self.assertRaisesRegex(ValueError, 'firmato'):
            l.confeziona(path, path, 'wheel')
        self.assertEqual(path.read_bytes(), original)

    def test_manca_solo_notice(self):
        notice = self.file.parent / 'NOTICE'
        notice.write_bytes(b'Attribution')
        self.data['crate']['demo-1.0']['avvisi'].append({
            'file': 'NOTICE', 'sha256': a.impronta(notice.read_bytes()), 'origine': 'pacchetto'})
        dest = self.cli()
        files = l.leggi(dest)
        del files['LICENSES/crate-rust/demo-1.0/NOTICE']
        with zipfile.ZipFile(dest, 'w') as z:
            for name, content in files.items():
                z.writestr(name, content)
        with self.assertRaises(ValueError):
            l.verifica(dest, 'cli')

    def test_grafo_obsoleto(self):
        with patch.object(a, 'grafo', return_value=({'wheel/new': []}, {})):
            with self.assertRaisesRegex(ValueError, 'obsoleto'):
                a.verifica(self.data)

    def test_testi_prodotti_non_relicenziati(self):
        with patch.object(a, 'ROOT', self.root):
            self.assertEqual(a.file_del_pacchetto({
                'manifest_path': str(self.root / 'crate/Cargo.toml'), 'nostro': True}), {})
            (self.root / 'LICENSE').write_bytes(b'Plenora family source-available terms')
            files = a.file_del_pacchetto({
                'manifest_path': str(self.root / 'crate/Cargo.toml'), 'nostro': True})
        self.assertEqual(files, {'LICENSE': b'Plenora family source-available terms'})

    def test_grafo_usa_solo_runtime_e_feature_wheel(self):
        packages = [{'id': 'p', 'name': 'demo', 'version': '1.0'}]
        commands = []

        def cargo(*args):
            commands.append(args)
            if args[0] == 'metadata':
                return json.dumps({'packages': packages, 'workspace_members': []})
            return 'demo v1.0\ndemo v1.0 (*)\n'

        with patch.object(a, 'cargo', side_effect=cargo):
            graphs, found = a.grafo()
        self.assertEqual(len(graphs), 4)
        self.assertEqual(list(found), ['demo-1.0'])
        for command in commands[1:]:
            self.assertIn('normal,no-proc-macro', command)
            self.assertEqual('--features' in command, 'plenora-data-py' in command)


class RegistroVersionato(unittest.TestCase):
    def test_testi_e_ragioni_versionati(self):
        data = a.registro()
        a.contenuti(data, incompleti=True)
        for key, item in data['crate'].items():
            if not item['avvisi']:
                self.assertTrue(item['mancante']['ragione'], key)
                if not item['nostro']:
                    self.assertTrue(item['mancante']['vcs']['git']['sha1'], key)


if __name__ == '__main__':
    unittest.main()
