#!/usr/bin/env python3
"""Collect verbatim notices for resolved x86_64 Linux Rust dependencies."""
import hashlib
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
HUSKARL_ARCHIVES = {
    ("huskarl", "0.11.11"): "1fb0b1712f346c3c31fbd025f2c54ff85b105305e15c82537bb745b27c971133",
    ("huskarl-core", "0.10.11"): "d813b281d25020dc232a6b965322da44b6bcc0a6fe3bbdec0953033596972afe",
    ("huskarl-macros", "0.4.2"): "22c230a201c0301aa0979e67c8dfa2e86fdf94ad72dce4f129bf7f906e04ed2e",
}
HUSKARL_NOTICES = {
    "LICENSE-APACHE": "635d5cf12047de0a8cb4840b3d6dcade5e2fe40ee752ba7a44edccd9cf9d9834",
    "LICENSE-MIT": "6726decf596ec579749be46b5ed41337c9550e4b35f155095d267b4e5c40b283",
}
HUSKARL_NOTICE_DIR = Path(__file__).resolve().with_name("rust-notice-supplements") / "huskarl"


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


def _huskarl_notices(package, source, candidates, checksum):
    name, version = package["name"], package["version"]
    if (package["source"] != "registry+https://github.com/rust-lang/crates.io-index"
            or package["license"] != "MIT OR Apache-2.0"):
        raise CollectionError(f"Huskarl notice supplement source or license mismatch for {name} {version}")
    if candidates:
        raise CollectionError(f"remove the unused Huskarl notice supplement for {name} {version}")

    # Cargo fetch retains the published archive beside the registry source cache.
    registry_src = source.parent.parent
    archive = registry_src.parent / "cache" / source.parent.name / f"{name}-{version}.crate"
    if (registry_src.name != "src" or source.name != f"{name}-{version}"
            or not archive.is_file() or hashlib.sha256(archive.read_bytes()).hexdigest() != checksum):
        raise CollectionError(f"Huskarl notice supplement requires the pinned crate archive for {name} {version}")

    notices = []
    for filename, digest in HUSKARL_NOTICES.items():
        path = HUSKARL_NOTICE_DIR / filename
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise CollectionError(f"Huskarl notice supplement is missing or changed: {filename}")
        notices.append((Path(filename), path))
    return notices


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
            checksum = HUSKARL_ARCHIVES.get((name, version))
            if checksum is not None:
                candidates = _huskarl_notices(package, source, candidates, checksum)
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
