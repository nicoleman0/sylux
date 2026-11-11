#!/usr/bin/env python3
"""Validate observed coverage, not titles, skipped tests or declared totals."""
import hashlib
import json
import re
import sys
from pathlib import Path

HARNESS_ROWS = ["HARNESS_SELF", "FIXTURE_INVENTORY", "DEFAULT_SURFACE", "BROWSER_TLS"]
PRODUCT_ROWS = [f"A{i:02}" for i in range(1, 14)] + [f"F{i:02}" for i in range(1, 9)]
# Deliberately explicit: deleting, ignoring or filtering a case cannot reduce coverage.
HARNESS_CASES = {
    "barrier_duplicate_timeout_unused_and_clock_clone",
    "barrier_rejects_release_before_observed_arrival",
    "cleanup_rejects_replaced_directory_even_with_original_marker",
    "cleanup_tracks_workers_and_rejects_symlink_and_hardlink_marker",
    "common::faults::tests::duplicate_arm_preserves_original_action",
    "common::faults::tests::faults_are_one_shot_and_scoped_to_controller",
    "common::faults::tests::missing_fault_reach_is_not_silent",
    "common::fixtures::tests::active_content_and_empty_fixture_semantics",
    "common::fixtures::tests::empty_stream_is_empty",
    "common::fixtures::tests::same_seed_has_same_stream_and_digest",
    "common::fixtures::tests::stream_never_exceeds_configured_buffer",
    "coverage_rejects_missing_and_blocked_rows",
    "deterministic_fixture_is_generated_in_bounded_chunks",
    "fixture_inventory_has_independent_digest_and_fresh_credentials",
    "fixture_root_is_unique_private_and_explicitly_cleanup_owned",
    "injected_clock_and_barrier_have_deterministic_boundary",
    "private_file_is_created_mode_0600",
    "real_loopback_tls_trust_san_isolation_restart_and_join",
    "sanitizer_drops_all_headers_and_bodies_including_unknown_keys",
}


BROWSER_CASES = {
    "trusted_assets", "trusted_api_routes", "wrong_san", "unrelated_ca", "plaintext_and_cookie_policy",
    "accessibility_keyboard_sign_in", "accessibility_control_names", "accessibility_rename_escape_focus",
    "accessibility_live_failure_retry", "storage_responsive_controls", "storage_listing_states",
    "storage_upload_download_roundtrip", "storage_delayed_upload_browsing", "storage_upload_cancel",
    "storage_lost_commit_response",
}


def validate_browser(folder, row):
    observation = json.loads(evidence_file(folder, row).read_text())
    if observation.get("schema_version") != 1 or observation.get("scope") != "fixture-only" or any(observation.get(k) is not True for k in ["joined", "restarted", "cleaned"]):
        raise ValueError("browser lifecycle incomplete")
    phases = observation.get("phases", [])
    if [p.get("phase") for p in phases] != ["initial", "restart"]:
        raise ValueError("browser phases missing or duplicated")
    for phase in phases:
        if phase.get("engine") != "158.0.8127.18" or phase.get("playwright") != "1.65.0" or not re.fullmatch(r"[0-9a-f]{64}", phase.get("executable_sha256", "")):
            raise ValueError("browser engine qualification differs from pin")
        cases = phase.get("cases", [])
        if len(cases) != len(BROWSER_CASES) or {c.get("case_id") for c in cases} != BROWSER_CASES or any(c.get("status") != "passed" for c in cases):
            raise ValueError("browser invocation missing, skipped, duplicated or failed")
        raw = evidence_file(folder, {"evidence": phase.get("raw"), "evidence_sha256": phase.get("raw_sha256")})
        events = [json.loads(line) for line in raw.read_text().splitlines()]
        actual = [{"case_id": e.get("case_id"), "status": e.get("status")} for e in events if e.get("event") == "case"]
        if actual != cases or len(events) != len(cases) + 2 or events[0].get("event") != "discovery" or len(events[0].get("cases", [])) != len(BROWSER_CASES) or set(events[0].get("cases", [])) != BROWSER_CASES or events[-1] != {"event": "end", "status": "passed"}:
            raise ValueError("browser invocation log differs from observation")
        engine_path = evidence_file(folder, {
            "evidence": phase.get("engine_file"),
            "evidence_sha256": phase.get("engine_sha256"),
        })
        engine = json.loads(engine_path.read_text())
        if any(engine.get(k) is not True for k in ["closed", "normal_tls_verification", "isolated_nss"]) or engine.get("engine") != phase["engine"] or engine.get("playwright") != phase["playwright"] or engine.get("executable_sha256") != phase["executable_sha256"]:
            raise ValueError("browser close/trust observation incomplete")


def evidence_file(folder, row):
    relative = row.get("evidence")
    if not isinstance(relative, str) or not relative or Path(relative).is_absolute() or ".." in Path(relative).parts:
        raise ValueError("PASS needs a relative evidence file")
    path = folder / relative
    if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(folder.resolve()):
        raise ValueError("evidence missing or outside run")
    if hashlib.sha256(path.read_bytes()).hexdigest() != row.get("evidence_sha256"):
        raise ValueError("stale or unhashed evidence")
    return path


def validate(report, mode, folder):
    if report.get("schema_version") != 1 or report.get("mode") != mode:
        raise ValueError("schema/mode mismatch")
    rows = report["rows"]
    ids = [row["row_id"] for row in rows]
    if len(ids) != len(set(ids)):
        raise ValueError("duplicate row_id")
    if set(ids) != set(HARNESS_ROWS + PRODUCT_ROWS):
        raise ValueError("missing or unknown registered row")
    by_id = {row["row_id"]: row for row in rows}
    for row in rows:
        if row.get("status") not in {"PASS", "FAIL", "BLOCKED", "NOT_RUN"}:
            raise ValueError("unknown status (skips are not passes)")
        if row["status"] in {"BLOCKED", "NOT_RUN"} and not str(row.get("blocker") or "").strip():
            raise ValueError("unexecuted row requires explicit blocker")
        if row["status"] == "PASS":
            if row["row_id"] in PRODUCT_ROWS:
                raise ValueError("row has no registered observation adapter yet; cannot accept PASS")
            evidence_file(folder, row)
            if row["row_id"] == "BROWSER_TLS":
                validate_browser(folder, row)
    if by_id["HARNESS_SELF"]["status"] == "PASS":
        raw = evidence_file(folder, by_id["HARNESS_SELF"]).read_text()
        observed = re.findall(r"^test ([\w:]+) \.\.\. (\w+)\s*$", raw, re.M)
        if len(observed) != len(HARNESS_CASES) or {name for name, _ in observed} != HARNESS_CASES or any(status != "ok" for _, status in observed):
            raise ValueError("Rust invocation missing, ignored, duplicated or failed")
        if not re.search(rf"test result: ok\. {len(HARNESS_CASES)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out", raw):
            raise ValueError("Rust summary does not prove unfiltered execution")
    required = HARNESS_ROWS[:-1] if mode == "rust-self" else HARNESS_ROWS
    if mode == "qualification":
        required = HARNESS_ROWS + PRODUCT_ROWS
    errors = [f"{r} {by_id[r]['status']}: {by_id[r].get('blocker') or 'no successful observation'}" for r in required if by_id[r]["status"] != "PASS"]
    if errors:
        raise ValueError("; ".join(errors))


def main():
    if len(sys.argv) != 3 or sys.argv[2] not in {"rust-self", "harness-self", "qualification"}:
        print("usage: test-s1-coverage.py REPORT.json rust-self|harness-self|qualification", file=sys.stderr)
        return 64
    try:
        path = Path(sys.argv[1])
        validate(json.loads(path.read_text()), sys.argv[2], path.parent)
        print(f"coverage validated: {sys.argv[2]} (not S1 acceptance)")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"coverage validation failed: {error}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    raise SystemExit(main())
