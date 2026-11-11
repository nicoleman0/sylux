#!/usr/bin/env python3
"""Collect verbatim notices for resolved x86_64 Linux Rust dependencies."""
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile

TARGET = "x86_64-unknown-linux-gnu"
NOTICE_PARTS = ("license", "copying", "notice", "copyright")


class CollectionError(RuntimeError):
    pass


def _safe_component(value):
    if not re.fullmatch(r"[A-Za-z0-9_.+-]+", value):
        raise CollectionError(f"unsafe package path component: {value!r}")
    return value


def _resolved_packages(metadata):
    resolve = metadata.get("resolve")
    if not isinstance(resolve, dict):
        raise CollectionError("cargo metadata has no resolved dependency graph")
    active = {node.get("id") for node in resolve.get("nodes", [])}
    packages = [p for p in metadata.get("packages", []) if p.get("id") in active and p.get("source") is not None]
    return sorted(packages, key=lambda p: (p["name"], p["version"]))


def collect(metadata, output):
    """Build a new output directory from cargo metadata; output must not exist."""
    output = Path(output)
    if output.exists() or output.is_symlink():
        raise CollectionError(f"output already exists: {output}")
    packages = _resolved_packages(metadata)
    if not packages:
        raise CollectionError("resolved graph contains no third-party packages")

    parent = output.parent
    parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".rust-notices-", dir=parent))
    index = []
    try:
        for package in packages:
            name = package.get("name")
            version = package.get("version")
            license_id = package.get("license")
            if not all(isinstance(v, str) and v for v in (name, version, license_id)):
                raise CollectionError(f"resolved package has incomplete identity or SPDX license: {name or '<unknown>'}")
            source_value = package.get("manifest_path")
            if not source_value:
                raise CollectionError(f"missing source manifest path for {name} {version}")
            source = Path(source_value).parent.resolve(strict=True)
            if not source.is_dir():
                raise CollectionError(f"source directory missing for {name} {version}")

            candidates = []
            for path in source.rglob("*"):
                if not any(part in path.name.lower() for part in NOTICE_PARTS):
                    continue
                if path.is_symlink() or path.is_file():
                    resolved = path.resolve(strict=True)
                    try:
                        relative = resolved.relative_to(source)
                    except ValueError as exc:
                        raise CollectionError(f"notice path escapes crate source for {name} {version}") from exc
                    if not resolved.is_file():
                        continue
                    candidates.append((relative, resolved))
            candidates.sort(key=lambda item: item[0].as_posix())
            if not candidates:
                raise CollectionError(f"no LICENSE/COPYING/NOTICE/COPYRIGHT files found for {name} {version}")

            crate_dir = Path(_safe_component(name)) / _safe_component(version)
            notice_paths = []
            for relative, source_file in candidates:
                rel_posix = PurePosixPath(relative.as_posix())
                if rel_posix.is_absolute() or ".." in rel_posix.parts:
                    raise CollectionError(f"unsafe notice path for {name} {version}")
                destination = stage / crate_dir / Path(*rel_posix.parts)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source_file, destination)
                notice_paths.append((crate_dir / Path(*rel_posix.parts)).as_posix())
            index.append({"name": name, "version": version, "license": license_id, "notices": notice_paths})

        index.sort(key=lambda p: (p["name"], p["version"]))
        (stage / "index.json").write_text(json.dumps(index, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        os.rename(stage, output)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def main(argv):
    if len(argv) != 1:
        raise CollectionError("usage: collect-rust-notices.py NEW_OUTPUT_DIRECTORY")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", TARGET],
        text=True,
    ))
    collect(metadata, argv[0])


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (CollectionError, OSError, subprocess.CalledProcessError, json.JSONDecodeError) as exc:
        print(f"rust notice collection failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
