"""Mutazioni della policy Python e dei metadati del wheel."""
import unittest
import check_python_version as guard


class PythonPolicyTests(unittest.TestCase):
    def test_repository_coerente(self):
        self.assertEqual(guard.check(), [])

    def test_riferimento_divergente(self):
        old = '3.' + '13'
        self.assertTrue(guard.reference_errors('fixture', 'Python >= ' + old, guard.declared()))
        self.assertTrue(guard.reference_errors('fixture', 'abi3-py' + old.replace('.', ''), guard.declared()))

    def test_setup_e_matrice_non_duplicano_la_fonte(self):
        source = "      - uses: actions/setup-python@pin\n        with:\n          python-version-file: '.python-version'\n"
        self.assertEqual(guard.workflow_errors(source), [])
        self.assertTrue(guard.workflow_errors(source.replace('python-version-file:', 'python-version:')))
        self.assertTrue(guard.workflow_errors(source + "        python: ['altro']\n"))

    def test_wheel_e_interprete_devono_concordare(self):
        version = guard.declared()
        runtime = tuple(map(int, version.split('.')))
        tags = ['cp' + version.replace('.', '') + '-abi3-win_amd64']
        self.assertEqual(guard.wheel_errors(version, '>=' + version, tags, runtime), [])
        self.assertTrue(guard.wheel_errors(version, '>=0', tags, runtime))
        self.assertTrue(guard.wheel_errors(version, '>=' + version, ['py3-none-any'], runtime))
        self.assertTrue(guard.wheel_errors(version, '>=' + version, tags, (0, 0)))


if __name__ == '__main__':
    unittest.main()
