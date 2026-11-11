#!/usr/bin/env python3
"""Regression for fail-closed execution of the acceptance verifier itself."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class VerifierExecutionPolicy(unittest.TestCase):
    def test_optimized_python_is_rejected_before_creating_evidence(self):
        (ROOT / '.local').mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / '.local', prefix='verifier-test-') as directory:
            output = Path(directory) / 'must-not-be-created'
            result = subprocess.run(
                [sys.executable, '-O', str(ROOT / 'tools/verify-dependency-locks.py'), str(output)],
                cwd=ROOT, capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('assertion checks must be enabled', result.stderr)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
