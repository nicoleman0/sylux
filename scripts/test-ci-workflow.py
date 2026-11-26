#!/usr/bin/env python3
import os
import re
import tomllib
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


def gate_script():
    source = (ROOT / '.github/workflows/ci.yml').read_text()
    gate = source.split('\n  check:\n', 1)[1]
    return '\n'.join(line[10:] for line in gate.split('        run: |\n', 1)[1].splitlines())


class RequiredChecks(unittest.TestCase):
    def run_gate(self, **overrides):
        env = dict(os.environ, SELECTION='success', FULL='true', AUTHORITY='true',
                   RUSTDOC='false', NATIVE_RESULT='success', WEB_RESULT='success',
                   TLS_RESULT='success', MODEL_RESULT='success', DOC_RESULT='skipped',
                   SEMGREP_RESULT='success')
        env.update(overrides)
        return subprocess.run(['bash', '-e', '-c', gate_script()], env=env,
                              capture_output=True, text=True)

    def test_release_artifact_download_and_concurrency_contract(self):
        source = (ROOT / '.github/workflows/beta-release.yml').read_text()
        self.assertIn('group: beta-release-${{ github.event_name }}', source,
                      'Separate dispatch and publication concurrency groups.')
        downloads = re.findall(r'uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093[^\n]*\n        with:\n((?:          [^\n]*\n)+)', source)
        identities = []
        for download in downloads:
            identity = re.search(r'artifact-ids: \$\{\{ needs\.([a-z-]+)\.outputs\.artifact \}\}', download)
            self.assertIsNotNone(identity, 'Download artifacts by the producing job’s ID.')
            identities.append(identity[1])
            self.assertIn('merge-multiple: true', download,
                          'ID downloads must extract directly into the verifier path.')
        self.assertCountEqual(identities, ['qualify'] * 4 + ['native-qualify'])
        self.assertIn("needs.native-qualify.result == 'success'", source)
        self.assertIn('path: release/docker', source)
        self.assertIn('path: release/native', source)

    def test_contract_targets_are_in_required_native_checks(self):
        manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())
        native = (ROOT / '.github/workflows/ci.yml').read_text().split('\n  native:\n', 1)[1].split('\n  web-smoke:', 1)[0]
        commands = native + (ROOT / 'scripts/check.sh').read_text()
        selected = set(re.findall(r'--test\s+(contract_[a-z_]+)', commands))
        required = {target['name'] for target in manifest.get('test', [])
                    if target['name'].startswith('contract_')}
        self.assertFalse(required - selected,
                         f'Add contract targets to required native CI checks: {sorted(required - selected)}')

    def test_full_change_requires_all_selected_results(self):
        self.assertEqual(self.run_gate().returncode, 0)
        for check in ('NATIVE_RESULT', 'WEB_RESULT', 'TLS_RESULT', 'MODEL_RESULT'):
            for result in ('failure', 'cancelled', 'skipped', ''):
                with self.subTest(check=check, result=result):
                    self.assertNotEqual(self.run_gate(**{check: result}).returncode, 0)

    def test_failed_selection_cannot_produce_a_green_gate(self):
        for result in ('failure', 'cancelled', 'skipped', ''):
            with self.subTest(result=result):
                self.assertNotEqual(self.run_gate(SELECTION=result, FULL='false',
                                                 AUTHORITY='false').returncode, 0)

    def test_comment_fast_path_allows_intentionally_skipped_jobs(self):
        self.assertEqual(self.run_gate(FULL='false', AUTHORITY='false',
                                      NATIVE_RESULT='skipped', WEB_RESULT='skipped',
                                      TLS_RESULT='skipped', MODEL_RESULT='skipped').returncode, 0)

    def test_semgrep_is_required_for_every_change(self):
        for full, authority in (('true', 'true'), ('false', 'false')):
            for result in ('failure', 'cancelled', 'skipped', ''):
                with self.subTest(full=full, result=result):
                    self.assertNotEqual(self.run_gate(FULL=full, AUTHORITY=authority,
                                                     SEMGREP_RESULT=result).returncode, 0)

    def test_rust_documentation_requires_its_selected_check(self):
        for result in ('failure', 'cancelled', 'skipped', ''):
            with self.subTest(result=result):
                self.assertNotEqual(self.run_gate(FULL='false', AUTHORITY='false',
                                                 RUSTDOC='true', DOC_RESULT=result).returncode, 0)
        self.assertEqual(self.run_gate(FULL='false', AUTHORITY='false',
                                      RUSTDOC='true', DOC_RESULT='success').returncode, 0)

    def test_model_only_change_requires_model_success(self):
        self.assertNotEqual(self.run_gate(FULL='false', MODEL_RESULT='skipped').returncode, 0)
        self.assertEqual(self.run_gate(FULL='false').returncode, 0)


if __name__ == '__main__':
    unittest.main()
