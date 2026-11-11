#!/usr/bin/env python3
"""Bounded T004 runner. rust-self is a subset; H0/browser and S1 remain distinct."""
import argparse
import hashlib
import http.client
import importlib.util
import json
import os
import re
import secrets
import selectors
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("coverage", ROOT / "scripts/test-s1-coverage.py")
assert spec is not None and spec.loader is not None
coverage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coverage)


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, ensure_ascii=False)
        stream.write("\n")


def default_surface(binary, folder):
    env = os.environ.copy()
    env.update(SYLUX_LISTEN="127.0.0.1:0", SYLUX_ASSETS=str(ROOT / "web/dist"), SYLUX_TEST_SEED=secrets.token_hex(32), SYLUX_TEST_FAULT="stage.before_write")
    process = subprocess.Popen([str(binary)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True)
    assert process.stderr is not None
    try:
        selector = selectors.DefaultSelector()
        selector.register(process.stderr, selectors.EVENT_READ)
        try:
            deadline = time.monotonic() + 10
            announcement = bytearray()
            while b"\n" not in announcement and len(announcement) < 1024:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise RuntimeError("default listener announcement timed out")
                chunk = os.read(process.stderr.fileno(), 1024 - len(announcement))
                if not chunk:
                    raise RuntimeError("default listener exited without announcement")
                announcement.extend(chunk)
            line = announcement.decode("utf-8", "replace")
        finally:
            selector.close()
        match = re.fullmatch(r"Sylux static assets listening on 127\.0\.0\.1:(\d+)\n", line)
        if not match:
            raise RuntimeError("default listener did not announce an owned loopback address")
        port = int(match[1])
        probes = []
        for path in ["/__test/seed", "/__test/faults", "/harness-status"]:
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
            try:
                connection.request("GET", path)
                response = connection.getresponse()
                if response.status != 404:
                    raise RuntimeError("default artifact exposes an unexpected harness route")
                probes.append({"method": "GET", "route": path, "status": response.status})
            finally:
                connection.close()
        write_json(folder / "default-surface.json", {"scope": "scaffold-only; T027/T034 must repeat against actual product routes", "transport": "scaffold HTTP, not a product TLS acceptance claim", "features": [], "artifact_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "probes": probes})
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        process.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=["rust-self", "harness-self", "qualification"], default="harness-self")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    os.chdir(ROOT)
    os.umask(0o077)
    # Fresh evidence only. No caller-provided credential directory and no overwrite.
    folder = args.evidence or Path("evidence/s1") / ("T004-" + secrets.token_hex(8))
    if not folder.is_absolute():
        folder = ROOT / folder
    evidence_parent = ROOT / "evidence"
    if ".." in folder.parts or not folder.is_relative_to(evidence_parent):
        parser.error("evidence must be a fresh child of this worktree's ignored evidence directory")
    current = ROOT
    for part in folder.relative_to(ROOT).parts:
        current /= part
        if current.is_symlink():
            parser.error("evidence must not contain symlink components")
    folder.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    folder.mkdir(mode=0o700, exist_ok=False)
    rows = {r: {"row_id": r, "status": "BLOCKED", "blocker": "required suite has not executed", "evidence": None} for r in coverage.HARNESS_ROWS + coverage.PRODUCT_ROWS}
    for row in coverage.PRODUCT_ROWS:
        rows[row]["blocker"] = "future product implementation/qualification (T005–T035); no product observation in T004"
    rows["BROWSER_TLS"]["blocker"] = "isolated NSS certutil trust provisioning and positive/negative browser suite pending; no TLS bypass allowed"
    commands = []
    report = {"schema_version": 1, "mode": args.mode, "rows": list(rows.values()), "commands": commands}
    failed = False
    env = os.environ.copy()
    env.update(S1_HARNESS_OUTPUT=str(folder), CARGO_BUILD_JOBS="1", CARGO_INCREMENTAL="0")

    def run(name, command, timeout=180):
        nonlocal failed
        log = folder / f"{name}.log"
        with log.open("xb") as output:
            child = subprocess.Popen(command, env=env, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = child.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
                code = 124
            except BaseException:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
                raise
        commands.append({"id": name, "argv": command, "exit_code": code, "raw": log.name})
        print(f"{name}: exit={code}", flush=True)
        if code:
            failed = True
            raise RuntimeError(f"{name} failed (see its private raw output)")
        return log

    def passed(row, path):
        rows[row].update(status="PASS", blocker=None, evidence=path.name, evidence_sha256=hashlib.sha256(path.read_bytes()).hexdigest())

    try:
        for tool in ["cargo", "node", "npm", "openssl"]:
            if not shutil.which(tool):
                raise RuntimeError(f"missing required tool: {tool}")
        if shutil.disk_usage(ROOT).free < 1024 ** 3:
            raise RuntimeError("less than 1 GiB safe free-space guard")
        report["base_commit"] = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        sources = [ROOT / "Cargo.toml", ROOT / "Cargo.lock"]
        for directory in ["src", "tests", "scripts"]:
            sources += sorted(p for p in (ROOT / directory).rglob("*") if p.is_file() and "__pycache__" not in p.parts)
        digest = hashlib.sha256()
        for path in sources:
            digest.update(str(path.relative_to(ROOT)).encode())
            digest.update(path.read_bytes())
        report["source_sha256"] = digest.hexdigest()
        run("web-build", ["npm", "--prefix", "web", "run", "build"])
        run("discovery", ["cargo", "test", "--locked", "--no-default-features", "--features", "test-harness", "--target-dir", "target/s1-harness", "--test", "harness_self", "--", "--list"])
        discovery = (folder / "discovery.log").read_text()
        if set(re.findall(r"^([\w:]+): test$", discovery, re.M)) != coverage.HARNESS_CASES:
            raise RuntimeError("Rust discovery differs from required registry")
        raw = run("rust-self", ["cargo", "test", "--locked", "--no-default-features", "--features", "test-harness", "--target-dir", "target/s1-harness", "--test", "harness_self", "--", "--test-threads=2"])
        passed("HARNESS_SELF", raw)
        inventory = folder / "fixture-inventory.json"
        files = json.loads(inventory.read_text())["files"]
        if len(files) != 8 or len({f["id"] for f in files}) != 8:
            raise RuntimeError("fixture inventory incomplete")
        passed("FIXTURE_INVENTORY", inventory)
        run("default-build", ["cargo", "build", "--release", "--locked", "--no-default-features", "--target-dir", "target/s1-production", "--bin", "sylux"], timeout=600)
        metadata = run("metadata", ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"])
        package = next(p for p in json.loads(metadata.read_text())["packages"] if p["name"] == "sylux")
        harness = next(t for t in package["targets"] if t["name"] == "harness_self")
        if package["features"].get("default") != [] or harness.get("required-features") != ["test-harness"]:
            raise RuntimeError("test target/feature isolation differs from H0 registration")
        default_surface(ROOT / "target/s1-production/release/sylux", folder)
        passed("DEFAULT_SURFACE", folder / "default-surface.json")
        run("coverage-negative-controls", ["python3", "scripts/test-s1-coverage.test.py"])
        run("browser-config-types", ["node", "web/node_modules/typescript/bin/tsc", "-p", "tests/browser/tsconfig.json"])
        run("browser-reporter-canary", ["node", "scripts/test-browser-reporter.mjs"])
        run("browser-output-controls", ["node", "scripts/test-browser-output.mjs"])
        if args.mode != "rust-self":
            certutil = env.get("S1_CERTUTIL") or shutil.which("certutil")
            if not certutil or not Path(certutil).is_file() or not os.access(certutil, os.X_OK):
                raise RuntimeError("approved certutil executable required via S1_CERTUTIL/PATH; runner never installs tools")
            env["S1_CERTUTIL"] = str(Path(certutil).resolve())
            browser_target = next(t for t in package["targets"] if t["name"] == "browser_fixture")
            if browser_target.get("required-features") != ["test-harness"]:
                raise RuntimeError("browser fixture target must require test-harness")
            run("browser-discovery", ["npm", "--silent", "--prefix", "web", "run", "test:browser", "--", "--list"])
            run("browser-fixture", ["cargo", "test", "--locked", "--no-default-features", "--features", "test-harness", "--target-dir", "target/s1-harness", "--test", "browser_fixture"], timeout=360)
            observation = folder / "browser-observation.json"
            coverage.validate_browser(folder, {"evidence": observation.name, "evidence_sha256": hashlib.sha256(observation.read_bytes()).hexdigest()})
            passed("BROWSER_TLS", observation)
        else:
            rows["BROWSER_TLS"]["blocker"] = "rust-self intentionally excludes real browser fixture lifecycle; run harness-self with approved certutil/browser"
    except (OSError, RuntimeError, ValueError, KeyError, StopIteration, subprocess.SubprocessError) as error:
        failed = True
        report["runner_error"] = str(error)
        # Exact command failures stay in local raw logs; no application bodies retained.
    finally:
        write_json(folder / "results.json", report)
    try:
        coverage.validate(report, args.mode, folder)
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(f"coverage incomplete: {error}", file=sys.stderr)
        return 2
    print(f"verified {args.mode} subset; browser/product status remains explicit in results.json")
    return 1 if failed else 0

if __name__ == "__main__":
    raise SystemExit(main())
