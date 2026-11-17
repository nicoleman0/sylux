#!/usr/bin/env python3
"""Disposable upload/hash, resume, RSS and paired PUT/append measurements.

Build current web assets first. Evidence is retained outside the repository.
"""

import subprocess
import json
import ssl
import http.client
import socket
import time
import hashlib
import pathlib
import threading
import tempfile
import urllib.parse
import sys

repo = pathlib.Path(__file__).resolve().parent.parent
build = subprocess.run(
    [
        "cargo",
        "test",
        "--locked",
        "--features",
        "test-harness",
        "--test",
        "performance_fixture",
        "--no-run",
        "--message-format=json",
    ],
    cwd=repo,
    check=True,
    capture_output=True,
    text=True,
)
artifacts = [
    json.loads(line) for line in build.stdout.splitlines() if line.startswith("{")
]
binary = next(
    row["executable"]
    for row in artifacts
    if row.get("target", {}).get("name") == "performance_fixture"
    and row.get("executable")
)
out = pathlib.Path(tempfile.mkdtemp(prefix="syl83-evidence-"))
print(out, flush=True)
server = subprocess.Popen(
    [binary, "--run", "100"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=open(out / "server.log", "w"),
    text=True,
    cwd=repo,
)
ready = json.loads(server.stdout.readline())
assert ready["event"] == "ready", ready
(out / "source.json").write_text(
    json.dumps(
        {
            "head": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=repo, text=True
            ).strip(),
            "diff_sha256": hashlib.sha256(
                subprocess.check_output(["git", "diff", "HEAD"], cwd=repo)
            ).hexdigest(),
            "assets_sha256": ready["embedded_assets_index_sha256"],
            "profile": "Cargo test profile",
            "platform": sys.platform,
        }
    )
)
port = urllib.parse.urlparse(ready["origin"]).port
# The fixture CA predates Python 3.15's extra AKI requirement; issuer and SAN still verify.
context = ssl.create_default_context(cafile=ready["ca"])
context.verify_flags &= ~ssl.VERIFY_X509_STRICT


class LocalHTTPS(http.client.HTTPSConnection):
    def connect(self):
        raw = socket.create_connection(("127.0.0.1", port), timeout=120)
        self.sock = context.wrap_socket(raw, server_hostname="fixture.test")


session = ready["sessions"][0]
headers = {
    "Cookie": "__Host-sylux=" + session["cookie"],
    "Origin": ready["origin"],
    "X-CSRF-Token": session["csrf"],
}


api_connection = LocalHTTPS("fixture.test", port, context=context, timeout=120)


def request(method, path, body=None, extra=None):
    c = api_connection
    c.request(method, path, body, dict(headers, **(extra or {})))
    r = c.getresponse()
    data = r.read()
    status = r.status
    assert 200 <= status < 300, (method, path, status, data[:500])
    return json.loads(data) if data else None


stop = False
rss = []


def sample():
    while not stop:
        x = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(server.pid)], capture_output=True, text=True
        )
        if x.stdout.strip():
            rss.append((time.monotonic(), int(x.stdout)))
        time.sleep(0.1)


threading.Thread(target=sample, daemon=True).start()
source = out / "four-gib-disposable.bin"
with source.open("wb") as f:
    f.truncate(4 * 1024**3)
results = []


def run(mode, length, name, interrupt=False):
    parent = request("GET", "/folders/children")["root"]
    body = {
        "parent_id": parent["id"],
        "parent_etag": parent["etag"],
        "name": name,
        "length": length,
    }
    ticket = request(
        "POST",
        "/mutation-tickets",
        json.dumps(dict(body, scope="upload_create")),
        {"Content-Type": "application/json"},
    )
    chunk = ticket["max_chunk_length"]
    request(
        "POST",
        "/uploads",
        json.dumps(body),
        {
            "Content-Type": "application/json",
            "If-None-Match": "*",
            "X-Mutation-ID": ticket["ticket"],
        },
    )
    id = ticket["mutation_id"]
    start = time.monotonic()
    before = len(rss)
    with source.open("rb") as f:
        if mode == "put":
            c = LocalHTTPS("fixture.test", port, context=context, timeout=120)
            c.putrequest("PUT", f"/uploads/{id}/content")
            for k, v in dict(
                headers,
                **{
                    "Content-Type": "application/octet-stream",
                    "Content-Length": str(length),
                    "X-Mutation-ID": ticket["ticket"],
                },
            ).items():
                c.putheader(k, v)
            c.endheaders()
            left = length
            while left:
                data = f.read(min(chunk, left))
                c.send(data)
                left -= len(data)
            r = c.getresponse()
            data = r.read()
            assert r.status == 200, (r.status, data)
            staged = json.loads(data)
            c.close()
        else:
            offset = 0
            while offset < length:
                data = f.read(min(chunk, length - offset))
                extra = {
                    "Content-Type": "application/octet-stream",
                    "X-Mutation-ID": ticket["ticket"],
                    "Upload-Offset": str(offset),
                    "Upload-Checksum": hashlib.sha256(data).hexdigest(),
                }
                if interrupt and offset == chunk:
                    c = LocalHTTPS("fixture.test", port, context=context, timeout=120)
                    c.putrequest("PATCH", f"/uploads/{id}/content")
                    for k, v in dict(
                        headers, **extra, **{"Content-Length": str(len(data))}
                    ).items():
                        c.putheader(k, v)
                    c.endheaders()
                    c.send(data[:4096])
                    c.close()
                    time.sleep(0.2)
                    state = request("GET", f"/uploads/{id}")
                    assert state["bytes_received"] == offset, state
                    results.append(
                        {
                            "case": "connection-killed-mid-chunk",
                            "offset": state["bytes_received"],
                            "writer_active": state["writer_active"],
                        }
                    )
                if interrupt and offset == 2 * chunk:
                    c = LocalHTTPS("fixture.test", port, context=context, timeout=120)
                    c.request(
                        "PATCH", f"/uploads/{id}/content", data, dict(headers, **extra)
                    )
                    assert c.sock.recv(1), "checkpoint response did not start"
                    c.close()
                    state = request("GET", f"/uploads/{id}")
                    assert state["bytes_received"] == offset + len(data), state
                    results.append(
                        {
                            "case": "response-discarded-after-checkpoint",
                            "offset": state["bytes_received"],
                        }
                    )
                else:
                    staged = request("PATCH", f"/uploads/{id}/content", data, extra)
                offset += len(data)
    elapsed = time.monotonic() - start
    committed = request(
        "POST",
        f"/uploads/{id}/commit",
        json.dumps({"sha256": staged["sha256"], "parent_etag": body["parent_etag"]}),
        {
            "Content-Type": "application/json",
            "If-None-Match": "*",
            "X-Mutation-ID": ticket["ticket"],
        },
    )
    assert request("GET", f"/uploads/{id}")["state"] == "committed"
    digest = hashlib.sha256()
    actual = 0
    downloadstart = time.monotonic()
    c = LocalHTTPS("fixture.test", port, context=context, timeout=120)
    c.request("GET", f"/entries/{committed['id']}/content", headers=headers)
    r = c.getresponse()
    assert r.status == 200, r.status
    while True:
        data = r.read(1024**2)
        if not data:
            break
        digest.update(data)
        actual += len(data)
    c.close()
    download_elapsed = time.monotonic() - downloadstart
    with source.open("rb") as f:
        expected = hashlib.sha256()
        left = length
        while left:
            data = f.read(min(chunk, left))
            expected.update(data)
            left -= len(data)
    assert (
        actual == length
        and digest.hexdigest() == expected.hexdigest() == staged["sha256"]
    )
    result = {
        "mode": mode,
        "length": length,
        "upload_seconds": elapsed,
        "upload_mib_s": length / 1024**2 / elapsed,
        "download_seconds": download_elapsed,
        "sha256": digest.hexdigest(),
        "server_rss_kib_min": min(x[1] for x in rss[before:]),
        "server_rss_kib_max": max(x[1] for x in rss[before:]),
    }
    results.append(result)
    (out / "results.json").write_text(json.dumps(results, indent=2))
    print(json.dumps(result), flush=True)


try:
    for mode in ["put", "append", "put", "append", "put", "append"]:
        run(mode, 256 * 1024**2, f"{mode}-{len(results)}.bin")
    run("append", 4 * 1024**3, "four-gib.bin", True)
finally:
    stop = True
    api_connection.close()
    server.stdin.write("stop\n")
    server.stdin.flush()
    server.wait(timeout=30)
    source.unlink(missing_ok=True)
    (out / "rss.json").write_text(json.dumps(rss))
    print("fixture exit", server.returncode, flush=True)
    assert server.returncode == 0, "fixture shutdown failed; retain its reported root"
