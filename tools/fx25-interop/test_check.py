import importlib.util, pathlib, unittest
HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('checker', HERE/'check.py')
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)
class ParserTests(unittest.TestCase):
    def test_actual_success(self):
        checker.check((HERE/'atest-success.txt').read_bytes())
    def test_failure_outputs(self):
        raw = (HERE/'atest-success.txt').read_bytes()
        for bad in [(HERE/'atest-failure.txt').read_bytes(),b'',raw.replace(b'FEC complete with no errors.',b'FEC failed. Too many errors.'),raw.replace(b'  63 6f',b'  62 6f'),raw.replace(b'coppa COP-114',b'wrong COP-114'),raw.replace(b'0x07 with 0 bit',b'0x06 with 0 bit')]:
            with self.assertRaises(ValueError): checker.check(bad)
