#!/usr/bin/env python3
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('checker', Path(__file__).with_name('check-account-authority.py'))
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class EvidenceChecks(unittest.TestCase):
    def test_source_drift_requires_review(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'accounts.rs'
            source.write_text('atomic authority change')
            coverage = {'files': {'accounts.rs': hashlib.sha256(source.read_bytes()).hexdigest()}}
            checker.check_sources(root, coverage)
            source.write_text('cached authority check')
            with self.assertRaisesRegex(ValueError, 'correspondence needs review'):
                checker.check_sources(root, coverage)
            source.unlink()
            with self.assertRaisesRegex(ValueError, 'correspondence needs review'):
                checker.check_sources(root, coverage)

    def test_negative_control_cannot_pass_or_fail_for_an_unrelated_reason(self):
        checker.check_result('negative', 12, 'Error: Invariant AtLeastOneAdmin is violated.')
        for code, output in [(0, 'Model checking completed. No error has been found.'),
                             (12, 'Invariant TypeOK is violated.'), (1, 'parse failed')]:
            with self.subTest(code=code, output=output), self.assertRaises(ValueError):
                checker.check_result('negative', code, output)

    def test_positive_requires_completed_success(self):
        checker.check_result('positive', 0, 'Model checking completed. No error has been found.')
        for code, output in [(0, 'Parsing file'), (12, 'Invariant AtLeastOneAdmin is violated.')]:
            with self.subTest(code=code), self.assertRaises(ValueError):
                checker.check_result('positive', code, output)

    def test_untrusted_jar_is_rejected_before_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            jar = Path(directory) / 'tlc.jar'
            jar.write_bytes(b'not the pinned checker')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                checker.prepare_jar(jar, {'sha256': '0' * 64}, False)


if __name__ == '__main__':
    unittest.main()
