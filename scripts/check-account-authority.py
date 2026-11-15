#!/usr/bin/env python3
"""Check the account model, its negative control and reviewed source correspondence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / 'verification/account-authority'


def check_sources(root, coverage):
    stale = [name for name, digest in coverage['files'].items()
             if not (root / name).is_file()
             or hashlib.sha256((root / name).read_bytes()).hexdigest() != digest]
    if stale:
        raise ValueError('Model correspondence needs review: ' + ', '.join(stale)
                         + '. Review the mapping and rerun its contracts before updating coverage.json.')


def prepare_jar(path, pin, download):
    if not path.exists():
        if not download:
            raise ValueError(f'TLC is missing: {path}. Use --download to fetch the pinned JAR.')
        path.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(pin['url'], timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != pin['sha256']:
            raise ValueError('Downloaded TLC checksum does not match verification/tlc.json')
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(data)
        try:
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    if hashlib.sha256(path.read_bytes()).hexdigest() != pin['sha256']:
        raise ValueError(f'TLC checksum mismatch: {path}')


def check_result(case, code, output):
    if case == 'positive':
        valid = code == 0 and 'Model checking completed. No error has been found.' in output
    else:
        valid = code == 12 and 'Invariant AtLeastOneAdmin is violated.' in output
    if not valid:
        raise ValueError(f'{case}: unexpected TLC result (exit {code}); inspect {case}.log')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jar', default=os.environ.get('TLA2TOOLS'))
    parser.add_argument('--java', default=os.environ.get('JAVA', 'java'))
    parser.add_argument('--download', action='store_true', help='fetch the pinned JAR if missing')
    parser.add_argument('--output', required=True, type=Path, help='new evidence directory')
    args = parser.parse_args()
    if not args.jar:
        parser.error('set TLA2TOOLS or --jar to the TLC JAR path')
    coverage = json.loads((MODEL / 'coverage.json').read_text())
    check_sources(ROOT, coverage)
    pin = json.loads((ROOT / 'verification/tlc.json').read_text())
    jar = Path(args.jar).expanduser().resolve()
    prepare_jar(jar, pin, args.download)
    version = subprocess.run([args.java, '-version'], capture_output=True, text=True, timeout=15)
    if version.returncode:
        raise ValueError('Java is not usable: ' + version.stderr.strip())
    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=False)
    (output_dir / 'java-version.log').write_text(version.stdout + version.stderr)
    results = {}
    for case in ('positive', 'negative'):
        command = [args.java, '-XX:+UseParallelGC', '-Xmx512m', '-cp', str(jar),
                   'tlc2.TLC', '-workers', '1', '-fp', '0', '-seed', '1',
                   '-metadir', str(output_dir / (case + '-states')),
                   '-config', str(MODEL / (case + '.cfg')), str(MODEL / 'AccountAuthority.tla')]
        try:
            result = subprocess.run(command, cwd=MODEL, capture_output=True, text=True, timeout=60)
        except subprocess.TimeoutExpired as error:
            captured = (error.stdout or b'') + (error.stderr or b'')
            (output_dir / (case + '.log')).write_bytes(captured)
            raise ValueError(f'{case}: TLC exceeded 60 seconds; exploration is incomplete') from error
        text = result.stdout + result.stderr
        (output_dir / (case + '.log')).write_text(text)
        check_result(case, result.returncode, text)
        results[case] = {'exit_code': result.returncode, 'expected_result': True}
        print(f'{case}: expected result (TLC exit {result.returncode})')
    summary = {'tlc': pin, 'source_correspondence': coverage, 'results': results,
               'scope': 'Bounded model and negative control; not proof of implementation or deployment.'}
    (output_dir / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(f'Evidence: {output_dir}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'account-authority check failed: {error}', file=sys.stderr)
        sys.exit(1)
