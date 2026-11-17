#!/usr/bin/env python3
"""Disposable TLS upload, resume, streaming digest and server RSS measurements.

Build current web assets first. The default run writes physical 4 GiB data.
Evidence remains outside the repository; only owned payload roots are removed.
"""

import argparse
import concurrent.futures
import hashlib
import http.client
import json
import os
import pathlib
import platform
import queue
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

REPO = pathlib.Path(__file__).resolve().parent.parent
BLOCK = 1024**2


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def file_hash(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        while data := stream.read(BLOCK):
            digest.update(data)
    return digest.hexdigest()


def create_source(path, length, seed):
    """SHAKE-256 counter blocks vary by file identity and absolute byte offset."""
    digest = hashlib.sha256()
    with path.open('xb') as stream:
        for offset in range(0, length, BLOCK):
            data = hashlib.shake_256(
                b'syl83-source-v1\0' + seed.encode() + b'\0' + offset.to_bytes(8, 'big')
            ).digest(min(BLOCK, length - offset))
            stream.write(data)
            digest.update(data)
    return digest.hexdigest()


def source_identity():
    paths = subprocess.check_output(
        ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=REPO
    ).split(b'\0')
    digest = hashlib.sha256()
    for name in sorted(set(filter(None, paths))):
        path = REPO / os.fsdecode(name)
        digest.update(len(name).to_bytes(8, 'big') + name)
        if path.is_symlink():
            digest.update(b'link\0' + os.fsencode(os.readlink(path)))
        elif path.is_file():
            digest.update(b'file\0' + bytes.fromhex(file_hash(path)))
        else:
            digest.update(b'missing\0')
    return {
        'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
        'source_tree_sha256': digest.hexdigest(),
        'diff_sha256': hashlib.sha256(subprocess.check_output(
            ['git', 'diff', '--binary', 'HEAD'], cwd=REPO)).hexdigest(),
        'status': subprocess.check_output(['git', 'status', '--porcelain'], cwd=REPO, text=True),
    }


def rss_summary(samples):
    require(bool(samples), 'No server RSS samples in measurement window; rerun with a longer sample window')
    return {'server_rss_kib_min': min(x[1] for x in samples),
            'server_rss_kib_max': max(x[1] for x in samples), 'rss_samples': len(samples)}


class Sampler:
    def __init__(self, pid):
        self.pid, self.samples, self.errors = pid, [], []
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.sample)

    def take(self):
        result = subprocess.run(['ps', '-o', 'rss=', '-p', str(self.pid)],
                                capture_output=True, text=True, timeout=5)
        if result.stdout.strip():
            self.samples.append((time.monotonic(), int(result.stdout.strip())))

    def sample(self):
        while not self.stop.is_set():
            try:
                self.take()
            except Exception as error:
                self.errors.append(str(error))
                return
            self.stop.wait(0.1)

    def finish(self):
        self.stop.set()
        self.thread.join(timeout=6)
        require(not self.thread.is_alive(), 'RSS sampler did not join')
        require(not self.errors, f'RSS sampling failed: {self.errors}')


class Client:
    def __init__(self, ready, session_index=0):
        self.port = urllib.parse.urlparse(ready['origin']).port
        self.context = ssl.create_default_context(cafile=ready['ca'])
        # Fixture CA lacks AKI required by Python 3.15 strict mode; CA and SAN still verify.
        self.context.verify_flags &= ~ssl.VERIFY_X509_STRICT
        session = ready['sessions'][session_index]
        self.folder = session['folder']
        self.headers = {'Cookie': '__Host-sylux=' + session['cookie'],
                        'Origin': ready['origin'], 'X-CSRF-Token': session['csrf']}
        self.api = self.connection()

    def connection(self):
        client = self

        class LocalHTTPS(http.client.HTTPSConnection):
            def connect(self):
                raw = socket.create_connection(('127.0.0.1', client.port), timeout=180)
                try:
                    self.sock = client.context.wrap_socket(raw, server_hostname='fixture.test')
                except BaseException:
                    raw.close()
                    raise

        return LocalHTTPS('fixture.test', self.port, context=self.context, timeout=180)

    def request(self, method, path, body=None, extra=None):
        try:
            self.api.request(method, path, body, dict(self.headers, **(extra or {})))
        except BrokenPipeError:
            # The server may answer and close before reading the whole body; report its answer.
            pass
        try:
            response = self.api.getresponse()
        except (BrokenPipeError, ConnectionError, http.client.HTTPException) as error:
            self.api.close()
            raise RuntimeError(f'{method} {path}: connection closed before a response: {error!r}') from error
        data = response.read()
        require(200 <= response.status < 300,
                f'{method} {path}: HTTP {response.status}: {data[:500]!r}')
        return json.loads(data) if data else None

    def root_child(self, entry_id):
        cursor = None
        while True:
            query = '?limit=500' + (f'&cursor={urllib.parse.quote(cursor)}' if cursor else '')
            page = self.request('GET', '/folders/children' + query)
            for item in page['items']:
                if item.get('id') == entry_id:
                    require(bool(item.get('etag')), f'Folder has no ETag: {item}')
                    return item
            cursor = page.get('next_cursor')
            require(page.get('has_more') and cursor, f'Seeded folder {entry_id} not found in root listing')

    def run(self, source, length, expected, mode, name, sampler, interrupt=False, barrier=None, own_folder=False):
        if own_folder:
            parent = self.root_child(self.folder)
        else:
            parent = self.request('GET', '/folders/children')['root']
        body = {'parent_id': parent['id'], 'parent_etag': parent['etag'], 'name': name, 'length': length}
        ticket = self.request('POST', '/mutation-tickets', json.dumps(dict(body, scope='upload_create')),
                              {'Content-Type': 'application/json'})
        mutation_headers = {'Content-Type': 'application/json', 'If-None-Match': '*',
                            'X-Mutation-ID': ticket['ticket']}
        self.request('POST', '/uploads', json.dumps(body), mutation_headers)
        chunk = ticket['max_chunk_length']
        require(0 < chunk <= 8 * BLOCK, f'Unexpected chunk bound: {chunk}')
        upload_id = ticket['mutation_id']
        if barrier:
            barrier.wait(timeout=60)
        start, before = time.monotonic(), len(sampler.samples)
        interruptions = []
        with source.open('rb') as stream:
            if mode == 'put':
                connection = self.connection()
                try:
                    connection.putrequest('PUT', f'/uploads/{upload_id}/content')
                    for key, value in dict(self.headers, **{
                        'Content-Type': 'application/octet-stream', 'Content-Length': str(length),
                        'X-Mutation-ID': ticket['ticket']}).items():
                        connection.putheader(key, value)
                    connection.endheaders()
                    left = length
                    while left:
                        data = stream.read(min(chunk, left))
                        require(bool(data), 'Source ended before declared length')
                        connection.send(data)
                        left -= len(data)
                    response = connection.getresponse()
                    data = response.read()
                    require(response.status == 200, f'PUT: {response.status}: {data[:500]!r}')
                    staged = json.loads(data)
                finally:
                    connection.close()
            else:
                offset = 0
                while offset < length:
                    data = stream.read(min(chunk, length - offset))
                    require(bool(data), 'Source ended before declared length')
                    extra = {'Content-Type': 'application/octet-stream', 'X-Mutation-ID': ticket['ticket'],
                             'Upload-Offset': str(offset), 'Upload-Checksum': hashlib.sha256(data).hexdigest()}
                    if interrupt and offset == chunk:
                        connection = self.connection()
                        try:
                            connection.putrequest('PATCH', f'/uploads/{upload_id}/content')
                            for key, value in dict(self.headers, **extra, **{'Content-Length': str(len(data))}).items():
                                connection.putheader(key, value)
                            connection.endheaders()
                            connection.send(data[:4096])
                        finally:
                            connection.close()
                        deadline = time.monotonic() + 75
                        while True:
                            state = self.request('GET', f'/uploads/{upload_id}')
                            require(state['bytes_received'] == offset, f'Interrupted offset changed: {state}')
                            if not state['writer_active']:
                                break
                            require(time.monotonic() < deadline, f'Interrupted writer did not release: {state}')
                            time.sleep(0.1)
                        interruptions.append({'case': 'connection-killed-mid-chunk', 'offset': offset})
                    if interrupt and offset == 2 * chunk:
                        connection = self.connection()
                        try:
                            connection.request('PATCH', f'/uploads/{upload_id}/content', data, dict(self.headers, **extra))
                            require(bool(connection.sock.recv(1)), 'Checkpoint response did not start')
                        finally:
                            connection.close()
                        # The checkpoint can land before the writer is released; wait as the browser does.
                        deadline = time.monotonic() + 30
                        while True:
                            state = self.request('GET', f'/uploads/{upload_id}')
                            if not state['writer_active']:
                                break
                            require(time.monotonic() < deadline, f'Writer did not release after checkpoint: {state}')
                            time.sleep(0.05)
                        require(state['bytes_received'] == offset + len(data), f'Lost acknowledgement: {state}')
                        interruptions.append({'case': 'response-discarded-after-checkpoint',
                                              'offset': state['bytes_received']})
                    else:
                        staged = self.request('PATCH', f'/uploads/{upload_id}/content', data, extra)
                    offset += len(data)
        elapsed = time.monotonic() - start
        sampler.take()
        upload_rss = rss_summary(sampler.samples[before:])
        committed = self.request('POST', f'/uploads/{upload_id}/commit', json.dumps({
            'sha256': staged['sha256'], 'parent_etag': body['parent_etag']}), mutation_headers)
        require(self.request('GET', f'/uploads/{upload_id}')['state'] == 'committed', 'Upload did not commit')
        digest, actual = hashlib.sha256(), 0
        download_start = time.monotonic()
        connection = self.connection()
        try:
            connection.request('GET', f"/entries/{committed['id']}/content", headers=self.headers)
            response = connection.getresponse()
            require(response.status == 200, f'Download: HTTP {response.status}')
            while data := response.read(BLOCK):
                digest.update(data)
                actual += len(data)
        finally:
            connection.close()
        require(actual == length, f'Download length: {actual}, expected {length}')
        require(digest.hexdigest() == expected == staged['sha256'],
                f'Digest mismatch: download={digest.hexdigest()}, source={expected}, staged={staged["sha256"]}')
        return {'mode': mode, 'name': name, 'length': length, 'upload_seconds': elapsed,
                'upload_mib_s': length / BLOCK / elapsed,
                'download_seconds': time.monotonic() - download_start, 'sha256': digest.hexdigest(),
                'source_sha256': expected, 'interruptions': interruptions, **upload_rss}


def stop_server(server):
    errors = []
    try:
        server.stdin.write('stop\n')
        server.stdin.flush()
    except (BrokenPipeError, OSError, ValueError) as error:
        errors.append(f'Fixture stop pipe: {error}')
    finally:
        try:
            server.stdin.close()
        except OSError as error:
            errors.append(f'Fixture stdin close: {error}')
    try:
        server.wait(timeout=30)
    except subprocess.TimeoutExpired:
        errors.append('Fixture graceful shutdown timed out')
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=10)
    if server.returncode != 0:
        errors.append(f'Fixture exited {server.returncode}')
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--quick', action='store_true', help='8 MiB paired/concurrent and 32 MiB resume smoke; not large-file evidence')
    args = parser.parse_args()
    identity = source_identity()
    build_command = ['cargo', 'test', '--locked', '--features', 'test-harness', '--test',
                     'performance_fixture', '--no-run', '--message-format=json']
    build_env = dict(os.environ, SYL48_SOURCE_SHA=identity['head'])
    build = subprocess.run(build_command, cwd=REPO, env=build_env, check=True, capture_output=True, text=True)
    require(source_identity() == identity, 'Source changed during build; wait for workers and rebuild exact head')
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.startswith('{')]
    artifact = next(row for row in artifacts if row.get('target', {}).get('name') == 'performance_fixture' and row.get('executable'))
    binary = pathlib.Path(artifact['executable'])
    out = pathlib.Path(tempfile.mkdtemp(prefix='syl83-evidence-'))
    print(out, flush=True)
    (out / 'build.log').write_text(build.stderr)
    metadata = {**identity, 'binary_sha256': file_hash(binary), 'binary': str(binary),
                'build_command': build_command, 'profile': artifact['profile'], 'quick': args.quick,
                'environment': {'platform': platform.platform(), 'machine': platform.machine(),
                    'python': sys.version, 'openssl': ssl.OPENSSL_VERSION,
                    'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
                    'cargo': subprocess.check_output(['cargo', '-V'], text=True),
                    'cpu_count': os.cpu_count(), 'SYL48_SOURCE_SHA': identity['head'],
                    'build_overrides': {key: os.environ[key] for key in (
                        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_TARGET',
                        'CARGO_TARGET_DIR', 'RUSTUP_TOOLCHAIN') if key in os.environ}},
                'source_generator': 'SHAKE-256 syl83-source-v1 seed + absolute offset, 1 MiB blocks'}
    (out / 'source.json').write_text(json.dumps(metadata, indent=2))
    fixture_parent = out / 'owned-fixture'
    fixture_parent.mkdir()
    server, sampler, reader = None, None, None
    sources, events, cleanup_errors = [], [], []
    original = None
    try:
        with (out / 'server.log').open('w') as log:
            server = subprocess.Popen([str(binary), '--run', '100'], stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=log, text=True, cwd=REPO,
                env=dict(os.environ, TMPDIR=str(fixture_parent), TMP=str(fixture_parent), TEMP=str(fixture_parent)))
        messages = queue.Queue()

        def read_events():
            try:
                for line in server.stdout:
                    events.append(line)
                    messages.put(line)
            finally:
                messages.put(None)

        reader = threading.Thread(target=read_events)
        reader.start()
        line = messages.get(timeout=120)
        require(line is not None, 'Fixture exited before readiness; see server.log')
        ready = json.loads(line)
        require(ready.get('event') == 'ready', f'Unexpected fixture event: {ready.get("event")}')
        require(ready['pid'] == server.pid, 'Fixture PID mismatch')
        require(pathlib.Path(ready['root']).resolve().is_relative_to(fixture_parent.resolve()), 'Fixture escaped owned temporary root')
        metadata.update({key: ready[key] for key in ('embedded_fixture_sha256', 'embedded_assets_index_sha256', 'source_at_build', 'schema_version')})
        require(ready['source_at_build'] == identity['head'], 'Embedded build SHA differs from requested head')
        (out / 'source.json').write_text(json.dumps(metadata, indent=2))
        sampler = Sampler(server.pid)
        sampler.thread.start()
        small, large = (8 * BLOCK, 32 * BLOCK) if args.quick else (256 * BLOCK, 4 * 1024**3)
        source = out / 'owned-source.bin'
        sources.append(source)
        # The paired sample is a prefix of the same physical, offset-varying 4 GiB file.
        large_hash = create_source(source, large, 'paired-and-resume')
        expected = hashlib.sha256()
        with source.open('rb') as stream:
            left = small
            while left:
                data = stream.read(min(BLOCK, left))
                require(bool(data), 'Source ended during prefix hash')
                expected.update(data)
                left -= len(data)
        metadata['source_files'] = [{'length': large, 'seed': 'paired-and-resume', 'sha256': large_hash},
                                    {'length': small, 'seed': 'paired-and-resume prefix', 'sha256': expected.hexdigest()}]
        results = []

        def record(result):
            results.append(result)
            (out / 'results.json').write_text(json.dumps(results, indent=2))
            print(json.dumps(result), flush=True)

        client = Client(ready)
        try:
            for index, mode in enumerate(['put', 'append'] * 3):
                record(client.run(source, small, expected.hexdigest(), mode, f'{mode}-{index}.bin', sampler))
            record(client.run(source, large, large_hash, 'append', 'resume.bin', sampler, interrupt=True))
        finally:
            client.api.close()
        source.unlink()
        barrier = threading.Barrier(3)
        concurrent_sources = []
        for index in range(3):
            path = out / f'owned-concurrent-{index}.bin'
            sources.append(path)
            seed = f'concurrent-{index}'
            digest = create_source(path, small, seed)
            concurrent_sources.append((path, digest))
            metadata['source_files'].append({'length': small, 'seed': seed, 'sha256': digest})
        (out / 'source.json').write_text(json.dumps(metadata, indent=2))

        def concurrent_upload(index):
            client = Client(ready, index)
            try:
                path, digest = concurrent_sources[index]
                return client.run(path, small, digest, 'append', f'concurrent-{index}.bin', sampler, barrier=barrier,
                                  own_folder=True)
            finally:
                client.api.close()

        before, started = len(sampler.samples), time.monotonic()
        # Each session uploads into its own seeded folder so commits do not race on one parent ETag.
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            futures = [pool.submit(concurrent_upload, index) for index in range(3)]
            concurrent_results = [future.result() for future in futures]
        record({'case': 'three-concurrent-distinct-uploads', 'concurrent_uploads': 3,
                'length_each': small, 'seconds': time.monotonic() - started,
                'rss_window': 'admission through streamed download verification',
                **(sampler.take() or rss_summary(sampler.samples[before:])), 'uploads': concurrent_results})
        require(source_identity() == identity, 'Source changed during measurement; results do not qualify current head')
    except BaseException as error:
        original = error
        raise
    finally:
        if sampler:
            try:
                sampler.finish()
            except Exception as error:
                cleanup_errors.append(str(error))
            try:
                (out / 'rss.json').write_text(json.dumps(sampler.samples))
            except OSError as error:
                cleanup_errors.append(str(error))
        if server:
            try:
                cleanup_errors.extend(stop_server(server))
            except Exception as error:
                cleanup_errors.append(f'Fixture shutdown: {error}')
        if reader:
            reader.join(timeout=5)
            if reader.is_alive():
                cleanup_errors.append('Fixture event reader did not join')
        if server and server.poll() is None:
            cleanup_errors.append(f'Fixture still running: PID {server.pid}; retain {fixture_parent}')
        else:
            for path in sources:
                try:
                    path.unlink(missing_ok=True)
                except OSError as error:
                    cleanup_errors.append(str(error))
            try:
                shutil.rmtree(fixture_parent)
            except OSError as error:
                cleanup_errors.append(str(error))
        try:
            (out / 'fixture-events.json').write_text(json.dumps(events))
            (out / 'cleanup.json').write_text(json.dumps({'errors': cleanup_errors,
                'server_exit': server.returncode if server else None, 'original_error': repr(original)}))
        except OSError as error:
            cleanup_errors.append(str(error))
        if cleanup_errors:
            message = '; '.join(cleanup_errors)
            if original is not None:
                original.add_note('Cleanup failures: ' + message)
            else:
                raise RuntimeError('Cleanup failures: ' + message)


if __name__ == '__main__':
    main()
