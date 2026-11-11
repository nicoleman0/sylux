#!/usr/bin/env python3
"""Validator negative controls. Generated parser inputs are NOT runtime evidence."""
import copy
import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("coverage", Path(__file__).with_name("test-s1-coverage.py"))
assert spec is not None and spec.loader is not None
coverage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coverage)


class CoverageControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sylux-coverage-")
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.raw = "".join(f"test {name} ... ok\n" for name in sorted(coverage.HARNESS_CASES))
        self.raw += f"test result: ok. {len(coverage.HARNESS_CASES)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
        self.log = self.folder / "validator-input.log"
        self.log.write_text(self.raw)
        self.report = {"schema_version": 1, "mode": "rust-self", "rows": [
            {"row_id": name, "status": "BLOCKED", "blocker": "unregistered future suite", "evidence": None}
            for name in coverage.HARNESS_ROWS + coverage.PRODUCT_ROWS
        ]}
        for name in coverage.HARNESS_ROWS[:-1]:
            self.row(name).update(status="PASS", blocker=None, evidence=self.log.name, evidence_sha256=self.digest())

    def row(self, name):
        return next(row for row in self.report["rows"] if row["row_id"] == name)

    def digest(self):
        return hashlib.sha256(self.log.read_bytes()).hexdigest()

    def rejected(self, message):
        with self.assertRaisesRegex(ValueError, message):
            coverage.validate(self.report, "rust-self", self.folder)

    def test_complete_parser_input_accepted_for_rust_subset_only(self):
        coverage.validate(self.report, "rust-self", self.folder)

    def test_pass_without_file_rejected(self):
        self.log.unlink()
        self.rejected("evidence missing")

    def test_missing_browser_row_rejected_even_in_subset(self):
        self.report["rows"].remove(self.row("BROWSER_TLS"))
        self.rejected("missing or unknown")

    def test_mode_mismatch(self):
        self.report["mode"] = "qualification"
        self.rejected("schema/mode mismatch")

    def test_unknown_extra_row(self):
        self.report["rows"].append({"row_id": "FAKE", "status": "PASS"})
        self.rejected("missing or unknown")

    def test_skipped_future_row(self):
        self.row("A01")["status"] = "SKIPPED"
        self.rejected("unknown status")

    def test_blocked_required_row_non_success(self):
        self.row("HARNESS_SELF").update(status="BLOCKED", blocker="unavailable")
        self.rejected("HARNESS_SELF BLOCKED")

    def test_duplicate_row(self):
        self.report["rows"].append(copy.deepcopy(self.report["rows"][0]))
        self.rejected("duplicate")

    def test_stale_evidence(self):
        self.log.write_text(self.raw + "changed")
        self.rejected("stale or unhashed")

    def test_hashed_but_filtered_test_output(self):
        self.log.write_text(self.raw.replace("... ok", "... ignored", 1))
        for name in coverage.HARNESS_ROWS[:-1]:
            self.row(name)["evidence_sha256"] = self.digest()
        self.rejected("invocation missing, ignored")

    def test_future_browser_or_product_pass_cannot_be_forged(self):
        for name in ["A01", "F08"]:
            old = copy.deepcopy(self.row(name))
            self.row(name).update(status="PASS", blocker=None, evidence=self.log.name, evidence_sha256=self.digest())
            self.rejected("no registered observation adapter")
            self.row(name).update(old)

    def test_browser_observation_requires_exact_executed_cases_and_lifecycle(self):
        # Parser input, never browser runtime evidence.
        self.report["mode"] = "harness-self"
        browser = self.folder / "browser-observation.json"
        observation = {"schema_version": 1, "scope": "fixture-only", "joined": True,
                       "restarted": True, "cleaned": True, "phases": []}
        names = {
            "trusted_assets", "trusted_api_routes", "wrong_san", "unrelated_ca", "plaintext_and_cookie_policy",
            "accessibility_keyboard_sign_in", "accessibility_control_names", "accessibility_rename_escape_focus",
            "accessibility_live_failure_retry", "storage_responsive_controls", "storage_listing_states",
            "storage_upload_download_roundtrip", "storage_delayed_upload_browsing", "storage_upload_cancel",
            "storage_lost_commit_response", "storage_pdf_preview",
        }
        engine_files = {}
        for phase in ["initial", "restart"]:
            cases = [{"case_id": name, "status": "passed"} for name in sorted(names)]
            import json
            log = self.folder / f"browser-{phase}-log"
            log.write_text(json.dumps({"event": "discovery", "cases": sorted(names)}) + "\n" + "".join(json.dumps({"event": "case", **c}) + "\n" for c in cases) + json.dumps({"event": "end", "status": "passed"}) + "\n")
            engine_dir = self.folder / f"browser-{phase}"
            engine_dir.mkdir()
            engine_path = engine_dir / "engine.json"
            engine_bytes = json.dumps({"engine": "158.0.8127.18", "playwright": "1.65.0", "executable_sha256": "a" * 64, "closed": True, "normal_tls_verification": True, "isolated_nss": True}).encode()
            engine_path.write_bytes(engine_bytes)
            engine_files[phase] = (engine_path, engine_bytes)
            observation["phases"].append({"phase": phase, "engine": "158.0.8127.18",
                "playwright": "1.65.0", "executable_sha256": "a" * 64,
                "engine_file": f"browser-{phase}/engine.json",
                "engine_sha256": hashlib.sha256(engine_bytes).hexdigest(),
                "raw": log.name, "raw_sha256": hashlib.sha256(log.read_bytes()).hexdigest(), "cases": cases})
        def save():
            import json
            browser.write_text(json.dumps(observation))
            self.row("BROWSER_TLS").update(status="PASS", blocker=None, evidence=browser.name,
                evidence_sha256=hashlib.sha256(browser.read_bytes()).hexdigest())
        save()
        coverage.validate(self.report, "harness-self", self.folder)
        engine_path, engine_bytes = engine_files["initial"]
        engine_path.write_bytes(engine_bytes + b" ")
        with self.assertRaisesRegex(ValueError, "stale or unhashed evidence"):
            coverage.validate(self.report, "harness-self", self.folder)
        engine_path.write_bytes(engine_bytes)
        symlink_target = engine_path.with_name("engine-target.json")
        symlink_target.write_bytes(engine_bytes)
        engine_path.unlink()
        engine_path.symlink_to(symlink_target)
        with self.assertRaisesRegex(ValueError, "evidence missing or outside run"):
            coverage.validate(self.report, "harness-self", self.folder)
        engine_path.unlink()
        engine_path.write_bytes(engine_bytes)
        observation["phases"][0]["cases"][0]["status"] = "skipped"
        save()
        with self.assertRaisesRegex(ValueError, "browser invocation"):
            coverage.validate(self.report, "harness-self", self.folder)
        for case in observation["phases"][0]["cases"]:
            case["status"] = "passed"
        observation["phases"][0]["cases"] = [
            case for case in observation["phases"][0]["cases"]
            if case["case_id"] != "trusted_api_routes"
        ]
        save()
        with self.assertRaisesRegex(ValueError, "browser invocation"):
            coverage.validate(self.report, "harness-self", self.folder)

    def test_unexplained_not_run(self):
        self.row("A01").update(status="NOT_RUN", blocker=None)
        self.rejected("explicit blocker")


if __name__ == "__main__":
    unittest.main()
