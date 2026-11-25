"""Tests for the deterministic Rust notice collector."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("collect-rust-notices.py")
spec = importlib.util.spec_from_file_location("collect_rust_notices", MODULE_PATH)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class RustNoticeCollectorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.crate = self.root / "registry" / "ring-0.17.14"
        (self.crate / "src").mkdir(parents=True)
        (self.crate / "Cargo.toml").write_text("[package]\nname='ring'\n")
        (self.crate / "LICENSE").write_bytes(b"ring license\x00\n")
        (self.crate / "LICENSE-other-bits").write_bytes(b"ring other bits\n")
        (self.crate / "src" / "LICENSE-BoringSSL").write_bytes(b"nested notice\n")
        self.metadata = {
            "packages": [
                {"id": "registry+ring 0.17.14", "name": "ring", "version": "0.17.14", "license": "Apache-2.0 AND ISC", "source": "registry+https://example.invalid", "manifest_path": str(self.crate / "Cargo.toml")},
                {"id": "registry+unused 1.0.0", "name": "unused", "version": "1.0.0", "license": "MIT", "source": "registry+https://example.invalid", "manifest_path": str(self.root / "absent" / "Cargo.toml")},
                {"id": "path+workspace", "name": "sylux", "version": "0.1.0", "license": "MIT", "source": None, "manifest_path": str(self.root / "Cargo.toml")},
            ],
            "resolve": {"nodes": [{"id": "registry+ring 0.17.14", "deps": []}]},
        }

    def tearDown(self):
        self.temp.cleanup()

    def test_nested_notices_bytes_and_index_are_deterministic(self):
        first, second = self.root / "one", self.root / "two"
        collector.collect(self.metadata, first)
        collector.collect(self.metadata, second)
        index_bytes = (first / "index.json").read_bytes()
        self.assertEqual(index_bytes, (second / "index.json").read_bytes())
        index = json.loads(index_bytes)
        self.assertEqual(index, [{
            "name": "ring", "version": "0.17.14", "license": "Apache-2.0 AND ISC",
            "notices": [
                "ring/0.17.14/LICENSE",
                "ring/0.17.14/LICENSE-other-bits",
                "ring/0.17.14/src/LICENSE-BoringSSL",
            ],
        }])
        for relative in index[0]["notices"]:
            self.assertEqual((first / relative).read_bytes(), (second / relative).read_bytes())
        self.assertEqual((first / "ring/0.17.14/LICENSE").read_bytes(), b"ring license\x00\n")

    def test_missing_notice_fails_with_package_identity(self):
        (self.crate / "LICENSE").unlink()
        (self.crate / "LICENSE-other-bits").unlink()
        (self.crate / "src/LICENSE-BoringSSL").unlink()
        out = self.root / "missing"
        with self.assertRaisesRegex(collector.CollectionError, "ring 0.17.14"):
            collector.collect(self.metadata, out)
        self.assertFalse(out.exists())

    def huskarl_fixture(self):
        source = "registry+https://github.com/rust-lang/crates.io-index"
        package = self.metadata["packages"][0]
        registry = self.root / "registry"
        crate = registry / "src" / "test-index" / "huskarl-0.11.11"
        crate.mkdir(parents=True)
        package.update(name="huskarl", version="0.11.11", license="MIT OR Apache-2.0",
                       source=source, manifest_path=str(crate / "Cargo.toml"))
        archive = registry / "cache" / "test-index" / "huskarl-0.11.11.crate"
        archive.parent.mkdir(parents=True)
        archive.write_bytes(b"test crate archive")
        return package, crate, archive

    def test_pinned_huskarl_receives_both_verbatim_notices(self):
        _, _, archive = self.huskarl_fixture()
        pins = {("huskarl", "0.11.11"): hashlib.sha256(archive.read_bytes()).hexdigest()}
        output = self.root / "supplemented"
        with patch.dict(collector.HUSKARL_ARCHIVES, pins):
            collector.collect(self.metadata, output)
        paths = json.loads((output / "index.json").read_bytes())[0]["notices"]
        self.assertEqual(paths, ["huskarl/0.11.11/LICENSE-APACHE", "huskarl/0.11.11/LICENSE-MIT"])
        for relative in paths:
            self.assertEqual((output / relative).read_bytes(),
                             (collector.HUSKARL_NOTICE_DIR / Path(relative).name).read_bytes())

    def test_huskarl_supplement_rejects_mismatched_identity_and_archive(self):
        package, _, _ = self.huskarl_fixture()
        output = self.root / "rejected"
        for field, value, error in (
            ("name", "lookalike", "no LICENSE"),
            ("version", "0.11.12", "no LICENSE"),
            ("source", "registry+https://example.invalid", "source or license mismatch"),
            ("license", "MIT", "source or license mismatch"),
        ):
            with self.subTest(field=field, value=value), patch.dict(package, {field: value}):
                with self.assertRaisesRegex(collector.CollectionError, error):
                    collector.collect(self.metadata, output)
                self.assertFalse(output.exists())
        with self.assertRaisesRegex(collector.CollectionError, "requires the pinned crate archive"):
            collector.collect(self.metadata, output)
        self.assertFalse(output.exists())

    def test_huskarl_supplement_rejects_unneeded_or_changed_notices(self):
        _, crate, archive = self.huskarl_fixture()
        pins = {("huskarl", "0.11.11"): hashlib.sha256(archive.read_bytes()).hexdigest()}
        output = self.root / "rejected"
        packaged_notice = crate / "LICENSE"
        packaged_notice.write_text("packaged license")
        with self.assertRaisesRegex(collector.CollectionError, "unused Huskarl notice supplement"):
            collector.collect(self.metadata, output)
        packaged_notice.unlink()
        with (patch.dict(collector.HUSKARL_ARCHIVES, pins),
              patch.dict(collector.HUSKARL_NOTICES, {"LICENSE-MIT": "wrong digest"})):
            with self.assertRaisesRegex(collector.CollectionError, "missing or changed: LICENSE-MIT"):
                collector.collect(self.metadata, output)
        self.assertFalse(output.exists())

    def test_notice_symlink_escape_fails(self):
        outside = self.root / "outside" / "COPYING"
        outside.parent.mkdir()
        outside.write_text("outside")
        (self.crate / "COPYING").symlink_to(outside)
        with self.assertRaisesRegex(collector.CollectionError, "escapes crate source"):
            collector.collect(self.metadata, self.root / "escape")

    def test_output_must_be_new(self):
        out = self.root / "existing"
        out.mkdir()
        with self.assertRaisesRegex(collector.CollectionError, "already exists"):
            collector.collect(self.metadata, out)

    def test_cli_invokes_cargo_and_writes_output(self):
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        metadata_path = self.root / "metadata.json"
        metadata_path.write_text(json.dumps(self.metadata))
        cargo = bin_dir / "cargo"
        cargo.write_text(f"#!/bin/sh\ncat {str(metadata_path)!r}\n")
        cargo.chmod(0o755)
        output = self.root / "cli-output"
        env = os.environ.copy()
        env["PATH"] = str(bin_dir) + os.pathsep + env.get("PATH", "")
        result = subprocess.run(
            [sys.executable, str(MODULE_PATH), str(output)],
            env=env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((output / "index.json").is_file())


if __name__ == "__main__":
    unittest.main()
