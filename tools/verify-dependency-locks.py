#!/usr/bin/env python3
"""Acceptance assertions for the S1 locks and native artifact provenance.

Run after cargo build --locked and npm --prefix web ci --ignore-scripts.
Does not install, clean, modify locks or run package lifecycle scripts.
Writes raw metadata/notices/hashes to the explicitly selected local output dir.
"""
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tomllib
import urllib.request
import uuid

if sys.flags.optimize:
    raise SystemExit('assertion checks must be enabled; optimized Python is unsupported')

ROOT = Path(__file__).resolve().parents[1]
os.chdir(ROOT)
OUT = Path(sys.argv[1] if len(sys.argv) == 2 else '.local/dependency-verification')
assert OUT.resolve().is_relative_to(ROOT / '.local') or 'evidence' in OUT.resolve().parts
OUT.mkdir(parents=True, exist_ok=True)

# No compiler, native-library or linker overrides may silently alter provenance.
overrides = sorted(k for k in os.environ if (
    k.startswith(('LIBSQLITE3_', 'SQLITE3_', 'SQLITE_MAX_', 'SQLCIPHER_', 'CFLAGS', 'CPPFLAGS', 'LDFLAGS', 'RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'CC_', 'AR_'))
    or k in {'CC', 'AR', 'ARFLAGS', 'HOST_CC', 'HOST_CFLAGS', 'HOST_AR', 'HOST_ARFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC', 'RUSTC_BOOTSTRAP'}
    or (k.startswith('CARGO_TARGET_') and k.endswith(('_LINKER', '_RUSTFLAGS')))
))
assert not overrides, f'unreviewed build overrides present (names only): {overrides}'

metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', 'x86_64-unknown-linux-gnu']))
(OUT / 'cargo-metadata.json').write_text(json.dumps(metadata, indent=2))
lock = tomllib.loads(Path('Cargo.lock').read_text())
manifest = tomllib.loads(Path('Cargo.toml').read_text())
assert manifest['package']['publish'] is False
assert manifest['package']['license'] == 'MIT' and 'license-file' not in manifest['package']
assert 'Permission is hereby granted, free of charge' in Path('LICENSE').read_text()
assert manifest['package']['rust-version'] == '1.99.1'
assert subprocess.check_output(['rustc', '--version'], text=True).startswith('rustc 1.99.1 ')
assert subprocess.check_output(['node', '--version'], text=True).strip() == 'v24.22.2'
assert subprocess.check_output(['npm', '--version'], text=True).strip() == '11.19.1'
assert 'host: x86_64-unknown-linux-gnu' in subprocess.check_output(['rustc', '-Vv'], text=True)
for name, dependency in manifest['dependencies'].items():
    assert dependency['version'].startswith('=') and not dependency['default-features']
    assert any(p['name'] == name and p['version'] == dependency['version'][1:] for p in lock['package'])
active_ids = {n['id'] for n in metadata['resolve']['nodes']}
features = {n['id']: n['features'] for n in metadata['resolve']['nodes']}
rust = []
sqlite = None
for package in metadata['packages']:
    if package['id'] not in active_ids or package['source'] is None:
        continue
    assert package['license'], f'unknown license: {package["name"]}'
    source = Path(package['manifest_path']).parent
    notices = {}
    for path in source.rglob('*'):
        if path.is_file() and any(x in path.name.lower() for x in ('license', 'copying', 'copyright', 'notice')):
            notices[str(path.relative_to(source))] = hashlib.sha256(path.read_bytes()).hexdigest()
    rust.append(dict(name=package['name'], version=package['version'], license=package['license'], rust_version=package['rust_version'], features=features[package['id']], notices=notices))
    if package['name'] == 'libsqlite3-sys':
        sqlite = source

assert {p['name'] for p in rust}.isdisjoint({'openssl', 'openssl-sys', 'aws-lc-rs', 'aws-lc-sys', 'sqlx'})
(OUT / 'rust-inventory.json').write_text(json.dumps(sorted(rust, key=lambda p: (p['name'], p['version'])), indent=2))

# Cargo validated registry downloads; independently bind archive -> source -> ELF.
assert sqlite is not None, 'SQLite source missing from resolved graph'
cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
crate = next((cargo_home / 'registry/cache').glob('*/libsqlite3-sys-0.39.1.crate'))
expected = next(p['checksum'] for p in lock['package'] if p['name'] == 'libsqlite3-sys')
assert hashlib.sha256(crate.read_bytes()).hexdigest() == expected
with tarfile.open(crate, 'r:gz') as archive:
    member = archive.extractfile('libsqlite3-sys-0.39.1/sqlite3/sqlite3.c')
    assert member is not None
    bundled_c = member.read()
assert bundled_c == (sqlite / 'sqlite3/sqlite3.c').read_bytes()
source_hash = hashlib.sha3_256(bundled_c).hexdigest()
request = urllib.request.Request('https://www.sqlite.org/releaselog/3_54_1.html', headers={'User-Agent': 'Sylux-S1-dependency-verification'})
release = urllib.request.urlopen(request, timeout=60).read()
(OUT / 'sqlite-release.html').write_bytes(release)
assert source_hash in release.decode(), 'bundled SQLite SHA3 differs from official release'
source_id = '2026-10-16 14:45:20 555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76'
assert source_id in bundled_c.decode() and source_id in release.decode()
wal = urllib.request.urlopen('https://www.sqlite.org/wal.html', timeout=60).read()
(OUT / 'sqlite-wal.html').write_bytes(wal)
assert b'3.51.3' in wal and b'3.50.7' in wal and b'3.44.6' in wal
build = subprocess.run(['cargo', 'build', '--locked', '--bin', 'sqlite-provenance', '--message-format=json'], check=True, capture_output=True, text=True)
(OUT / 'cargo-build-artifacts.jsonl').write_text(build.stdout)
(OUT / 'cargo-build-stderr.txt').write_text(build.stderr)
messages = [json.loads(line) for line in build.stdout.splitlines()]
binary = Path(next(m['executable'] for m in messages if m['reason'] == 'compiler-artifact' and m['target']['name'] == 'sqlite-provenance'))
native = subprocess.check_output(['readelf', '-d', str(binary)], text=True)
symbols = subprocess.check_output(['nm', '--defined-only', str(binary)], text=True)
(OUT / 'elf-dynamic.txt').write_text(native)
(OUT / 'elf-defined-symbols.txt').write_text(symbols)
assert 'sqlite' not in native.lower()
assert re.search(r'\b[Tt] sqlite3_sourceid$', symbols, re.M)
assert re.search(r'\b[Tt] sqlite3_libversion$', symbols, re.M)
fixture = OUT / f'probe-{uuid.uuid4().hex}.sqlite3'
result = subprocess.run([str(binary), str(fixture)], check=True, capture_output=True, text=True)
runtime = json.loads(result.stdout)
assert runtime['sqlite_source_id'] == source_id and runtime['sqlite_version'] == '3.54.1'
assert runtime['journal_mode'] == 'wal' and runtime['synchronous'] == 2 and runtime['foreign_keys'] == 1
(OUT / 'sqlite-runtime.json').write_text(result.stdout)
# Refusal must not touch an existing database, including its metadata.
before = fixture.read_bytes()
refusal = subprocess.run([str(binary), str(fixture)], capture_output=True, text=True)
assert refusal.returncode != 0 and fixture.read_bytes() == before
(OUT / 'existing-database-refusal.txt').write_text(refusal.stderr)
sqlite_out = Path(next(m['out_dir'] for m in messages if m['reason'] == 'build-script-executed' and 'libsqlite3-sys@0.39.1' in m['package_id']))
static_outputs = [sqlite_out.parent / 'output']
assert 'cargo:rustc-link-lib=static=sqlite3' in static_outputs[0].read_text()
for i, path in enumerate(static_outputs):
    (OUT / f'sqlite-build-output-{i}.txt').write_text(path.read_text())

npm_lock = json.loads(Path('web/package-lock.json').read_text())
npm_manifest = json.loads(Path('web/package.json').read_text())
assert npm_manifest['private'] is True and 'license' not in npm_manifest
assert npm_lock['lockfileVersion'] == 3
assert npm_lock['packages']['']['devDependencies'] == npm_manifest['devDependencies']
npm = []
native_npm = []
npm_cache = Path(subprocess.check_output(['npm', 'config', 'get', 'cache'], text=True).strip())
for location, package in npm_lock['packages'].items():
    if not location:
        continue
    name = location.rsplit('node_modules/', 1)[1]
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:[-+][\w.-]+)?', package['version'])
    if package.get('inBundle'):
        # Shipped inside the parent's archive, so the parent's integrity hash covers it.
        assert location.startswith('node_modules/@tailwindcss/oxide-wasm32-wasi/node_modules/')
    else:
        assert package.get('integrity', '').startswith('sha512-')
        assert package.get('resolved', '').startswith('https://registry.npmjs.org/')
    license = package.get('license')
    # Published without a license field; the shipped LICENSE file is checked below.
    unlabelled = name == 'svelte-toolbelt' and license is None
    assert unlabelled or license in {'MIT', 'Apache-2.0', 'BSD-3-Clause', 'ISC', 'MPL-2.0', '0BSD', 'OFL-1.1'}, f'unreviewed npm license: {name}: {license}'
    if license == 'OFL-1.1':
        assert name in {'@fontsource/poppins', '@fontsource/jetbrains-mono'}
        assert package['version'] == '5.3.0'
    if license == '0BSD':
        assert name == 'tslib'
    if license == 'MPL-2.0':
        assert name == 'lightningcss' or name.startswith('lightningcss-')
        assert package['version'] == '1.33.0'
    installed = Path('web') / location
    notices = {}
    if installed.exists():
        installed_package = json.loads((installed / 'package.json').read_text())
        assert installed_package['version'] == package['version']
        assert installed_package.get('license') == license
        if license == 'OFL-1.1':
            notice = Path('web/public/licenses') / (name.removeprefix('@fontsource/') + '.txt')
            assert notice.read_bytes() == (installed / 'LICENSE').read_bytes()
        if unlabelled:
            assert 'MIT License' in (installed / 'LICENSE').read_text()
        for path in installed.rglob('*'):
            if path.is_file() and any(x in path.name.lower() for x in ('license', 'copying', 'copyright', 'notice')):
                notices[str(path.relative_to(installed))] = hashlib.sha256(path.read_bytes()).hexdigest()
        for node in installed.glob('*.node'):
            # Verify the actual executable bytes against the SHA512-locked publisher archive.
            digest = base64.b64decode(package['integrity'].removeprefix('sha512-')).hex()
            cached = npm_cache / '_cacache/content-v2/sha512' / digest[:2] / digest[2:4] / digest[4:]
            archive_bytes = cached.read_bytes() if cached.exists() else urllib.request.urlopen(package['resolved'], timeout=60).read()
            integrity = 'sha512-' + base64.b64encode(hashlib.sha512(archive_bytes).digest()).decode()
            assert integrity == package['integrity']
            with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode='r:gz') as archive:
                member = archive.extractfile('package/' + node.name)
                assert member is not None
                assert member.read() == node.read_bytes()
            (OUT / (name.replace('/', '_').replace('@', '') + '.tgz')).write_bytes(archive_bytes)
            native_npm.append(dict(name=name, version=package['version'], license=license, archive=package['resolved'], integrity=integrity, binary_sha256=hashlib.sha256(node.read_bytes()).hexdigest(), notices=notices))
    npm.append(dict(name=name, version=package['version'], license=license, installed=installed.exists(), notices=notices, has_install_script=package.get('hasInstallScript', False)))
assert {p['name'] for p in native_npm} == {'@rolldown/binding-linux-x64-gnu', '@tailwindcss/oxide-linux-x64-gnu', 'lightningcss-linux-x64-gnu'}
(OUT / 'npm-inventory.json').write_text(json.dumps(npm, indent=2))
(OUT / 'npm-native-provenance.json').write_text(json.dumps(native_npm, indent=2))
summary = dict(rust_lock_packages=len(lock['package']) - 1, rust_target_packages=len(rust), npm_lock_packages=len(npm), npm_installed_packages=sum(p['installed'] for p in npm), npm_native=native_npm, sqlite_crate_sha256=expected, sqlite_source_sha3_256=source_hash, executable_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), sqlite_runtime=runtime)
(OUT / 'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary, indent=2))
