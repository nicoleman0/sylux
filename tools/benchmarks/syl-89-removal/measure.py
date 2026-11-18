#!/usr/bin/env python3
import argparse
import base64
from contextlib import contextmanager
import hashlib
import http.client
import json
import os
from pathlib import Path
import queue
import resource
import shutil
import signal
import sqlite3
import ssl
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser()
parser.add_argument("mode", choices=("measure", "diagnose", "smoke", "stress", "review"))
parser.add_argument("--repeats", type=int, default=5)
parser.add_argument("--idle-seconds", type=float, default=6.3)
parser.add_argument("--sample", action="store_true")
args = parser.parse_args()
home = Path(os.environ["SYL89_EVIDENCE_DIR"]).resolve()
output = Path(tempfile.mkdtemp(prefix=f"{args.mode}.", dir=home))
print(f"evidence={output}", flush=True)
binary = home / "sylux"
cert, key = output / "cert.pem", output / "key.pem"
with (output / "openssl.log").open("wb") as log:
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
                    "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1",
                    "-keyout", str(key), "-out", str(cert)], check=True, stdout=log, stderr=log)
context = ssl.create_default_context(cafile=str(cert))
ORIGIN = "https://localhost"
ENTITY_ROOT, OWNER, SID, FOLDER, OPERATION = [f"{n:032x}" for n in range(1, 6)]
DIGEST = hashlib.sha256(b"").digest()
SECRET, CSRF = bytes([11]) * 32, bytes([24]) * 32

def token(secret):
    return base64.urlsafe_b64encode(secret).decode().rstrip("=")

def emit(value):
    print(json.dumps(value), flush=True)

def dbpath(store):
    return store / "metadata/sylux.sqlite3"

@contextmanager
def connect(store):
    db = sqlite3.connect(dbpath(store))
    try:
        with db:
            db.execute("PRAGMA foreign_keys=ON")
            db.execute("PRAGMA synchronous=FULL")
            yield db
    finally:
        db.close()

def entry(eid, parent, kind, name, state="active"):
    return (eid, OWNER, parent, kind, name, name.encode(), state)

def seed(store, count, layout, library=0):
    store.mkdir(mode=0o700)
    initialized = subprocess.run([binary, "init", "--root", store],
                                 capture_output=True, text=True, timeout=30, check=True)
    assert json.loads(initialized.stderr.splitlines()[-1]) == {"contract_event": "init.complete"}
    now = int(time.time())
    tree = layout == "folder"
    insert_entry = """INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,
                       name_key,name_key_version,state) VALUES(?,?,?,?,?,?,1,?)"""
    entries = [f"{n + 10_000:032x}" for n in range(count)]
    blobs = [f"{(n % 256):02x}{n + 1:030x}" for n in range(count)]
    revisions = [f"{n + 100_000:032x}" for n in range(count)]
    with connect(store) as db:
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?,'measure-owner','administrator',1,0)", (OWNER,))
        db.execute(insert_entry, entry(ENTITY_ROOT, None, "folder", ""))
        db.execute("""INSERT INTO session(session_id,account_id,credential_epoch,verifier,
                      csrf_verifier,created_at,idle_expires_at,absolute_expires_at)
                      VALUES(?,?,0,?,?,?,?,?)""",
                   (SID, OWNER, hashlib.sha256(SECRET).digest(), hashlib.sha256(CSRF).digest(),
                    now, now + 7200, now + 14400))
        if count and tree:
            db.execute(insert_entry, entry(FOLDER, ENTITY_ROOT, "folder", "removed-folder"))
        db.executemany(insert_entry, (
            entry(eid, FOLDER if tree else ENTITY_ROOT, "file", f"file-{i:06d}",
                  "active" if tree else "deleted") for i, eid in enumerate(entries)))
        db.executemany("INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?,?,0,?)",
                       ((bid, bid[:2], DIGEST) for bid in blobs))
        db.executemany("""INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,
                          committed_at,actor_account_id) VALUES(?,?,?,0,?,1,?)""",
                       ((rid, eid, bid, DIGEST, OWNER) for rid, eid, bid in zip(revisions, entries, blobs)))
        db.executemany("UPDATE entry SET current_revision_id=? WHERE entry_id=?", zip(revisions, entries))
        if count and tree:
            db.execute("UPDATE entry SET state='deleted' WHERE entry_id=?", (FOLDER,))
        # Review: unrelated live library files owned by the same account.
        lib_e = [f"{n + 2_000_000:032x}" for n in range(library)]
        lib_b = [f"{(n % 256):02x}{n + 3_000_000:030x}" for n in range(library)]
        lib_r = [f"{n + 4_000_000:032x}" for n in range(library)]
        db.executemany(insert_entry, (entry(eid, ENTITY_ROOT, "file", f"lib-{i:06d}") for i, eid in enumerate(lib_e)))
        db.executemany("INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?,?,0,?)", ((b, b[:2], DIGEST) for b in lib_b))
        db.executemany("""INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,
                          committed_at,actor_account_id) VALUES(?,?,?,0,?,1,?)""",
                       ((r, e, b, DIGEST, OWNER) for r, e, b in zip(lib_r, lib_e, lib_b)))
        db.executemany("UPDATE entry SET current_revision_id=? WHERE entry_id=?", zip(lib_r, lib_e))
        blobs = blobs + lib_b
        if count and layout == "history":
            for chunk in range(0, count, 1000):
                operation = f"{chunk + 7:032x}"
                db.execute("""INSERT INTO removal(operation_id,owner_account_id,confirmation_hash,
                              removed_count,committed_at) VALUES(?,?,?,?,1)""",
                           (operation, OWNER, DIGEST, min(count - chunk, 1000)))
                db.executemany("INSERT INTO removal_member VALUES(?,?)",
                               ((eid, operation) for eid in entries[chunk:chunk+1000]))
                db.executemany("INSERT INTO removal_cleanup VALUES(?,?,'pending')",
                               ((bid, operation) for bid in blobs[chunk:chunk+1000]))
            db.execute("UPDATE blob SET lifecycle='deleted'")
            db.execute("UPDATE removal_cleanup SET state='complete'")
            assert db.execute("SELECT count(*) FROM removal_cleanup WHERE state='complete'").fetchone() == (count,)
        assert db.execute("PRAGMA quick_check").fetchall() == [("ok",)]
        assert db.execute("PRAGMA foreign_key_check").fetchall() == []
        assert db.execute("SELECT count(*) FROM blob").fetchone() == (count + library,)
    if layout != "history":
        (store / "payloads").mkdir(mode=0o700)
        for bid in blobs:
            shard = store / "payloads" / bid[:2]
            shard.mkdir(mode=0o700, exist_ok=True)
            (shard / bid).touch(mode=0o600, exist_ok=False)
    return entries

class Server:
    def __init__(self, store, label, *, breakdown=False):
        self.events = []
        self.pipe = queue.Queue()
        self.log = (output / f"{label}.log").open("w")
        self.start = time.monotonic_ns()
        self.cpu_start = resource.getrusage(resource.RUSAGE_CHILDREN)
        self.proc = subprocess.Popen([binary, "serve", "--root", store, "--listen", "127.0.0.1:0",
                                      "--tls-cert", cert, "--tls-key", key, "--public-origin", ORIGIN],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
                                     env={**os.environ, **({"SYL89_PREPARE_BREAKDOWN": "1"} if breakdown else {})})
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.proc.stderr:
            now = time.monotonic_ns()
            self.log.write(f"{now - self.start} {line}")
            self.events.append((now, line.strip()))
            self.pipe.put(self.events[-1])
        self.pipe.put(None)

    def ready(self):
        addr = None
        while True:
            item = self.pipe.get(timeout=60)
            assert item is not None, "server exited before ready"
            now, line = item
            if line.startswith("syl89 ready_addr="):
                addr = int(line.split(":")[-1])
            if line.startswith("{") and json.loads(line)["contract_event"] == "listener.bind":
                assert addr is not None
                return now, addr

    def close(self, expected=0):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
        try:
            result = self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            raise
        finally:
            self.reader.join(timeout=3)
            assert not self.reader.is_alive()
            self.log.close()
        assert result == expected, result
        cpu = resource.getrusage(resource.RUSAGE_CHILDREN)
        return (cpu.ru_utime - self.cpu_start.ru_utime + cpu.ru_stime - self.cpu_start.ru_stime) * 1e9

    def metrics(self, key):
        result = []
        for stamp, line in self.events:
            if line.startswith(f"syl89 {key}="):
                fields = dict(part.split("=", 1) for part in line.split()[1:])
                result.append((stamp, fields))
        return result

    def metric(self, key):
        [(stamp, fields)] = self.metrics(key)
        return int(fields[key])

def request(client, method, path, data=None):
    headers = {"Cookie": f"__Host-sylux={SID}.{token(SECRET)}", "Origin": ORIGIN,
               "X-CSRF-Token": token(CSRF), "Content-Type": "application/json"}
    client.request(method, path, json.dumps(data) if data is not None else None, headers)
    response = client.getresponse()
    body = response.read()
    assert response.status == 200, (response.status, body)
    return json.loads(body)

def sql_plans(store):
    source = home / "source.txt"
    checkout = Path(source.read_text().split(maxsplit=1)[1].strip())
    import re
    texts = [(checkout / f).read_text() for f in (
        "src/persistence/cleanup.rs", "src/persistence/reconcile.rs", "src/domain/removal.rs")]
    with connect(store) as db, (output / "query-plans.txt").open("w") as log:
        for file, fragments in zip(texts, (
            ("SELECT blob_id FROM removal_cleanup", "SELECT EXISTS(SELECT 1 FROM removal_cleanup"),
            ("SELECT blob_id FROM blob WHERE lifecycle", "SELECT EXISTS(SELECT 1 FROM removal_cleanup"),
            ("WITH RECURSIVE tree(id)", "WITH RECURSIVE ancestors(id,parent_id", "SELECT entry_id FROM entry WHERE owner_account_id",))):
            for fragment in fragments:
                for sql in re.findall(r'"(' + re.escape(fragment) + r'[^"]+)"', file):
                    refs = [int(s) for s in re.findall(r"\?(\d+)", sql)]
                    plan = db.execute("EXPLAIN QUERY PLAN " + sql, [""] * max(refs, default=0)).fetchall()
                    print(sql, plan, sep="\n", file=log)

def startup(store, label, idle=0.0, diagnostic=False):
    server = Server(store, label)
    profile = None
    if diagnostic and args.sample:
        profile = subprocess.Popen(["sample", str(server.proc.pid), "4", "1",
                                    "-file", str(output / f"{label}.sample.txt")],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        ready, _ = server.ready()
        if idle:
            time.sleep(idle)
    finally:
        # Keep sampling CPU separate from the reported runs.
        if profile:
            assert profile.wait(timeout=15) == 0
        cpu_ns = server.close()
    [(audit_time, audit)] = server.metrics("audit_ns")
    assert audit["valid"] == "true"
    assert audit_time < ready
    assert all("startup.failed" not in line and "removal.cleanup_pending" not in line
               for _, line in server.events)
    record = {"stage": "startup", "label": label, "ready_ns": ready - server.start,
              "reconcile_ns": server.metric("reconcile_ns"), "audit_ns": int(audit["audit_ns"]),
              "child_total_cpu_ns": cpu_ns}
    if idle:
        passes = [(stamp, fields) for stamp, fields in server.metrics("cleanup_pass_ns") if stamp > ready]
        lookups = [(stamp, fields) for stamp, fields in server.metrics("cleanup_lookup_ns") if stamp > ready]
        assert len(passes) == len(lookups) == int(idle), (passes, lookups)
        assert all(fields["candidates"] == "0" for _, fields in lookups), lookups
        record["idle"] = [
            {"trigger_ns": stamp - ready, "lookup_ns": int(lookup["cleanup_lookup_ns"]),
             "reader_ns": int(lookup["reader_ns"]), "pass_ns": int(run["cleanup_pass_ns"])}
            for (stamp, run), (_, lookup) in zip(passes, lookups)]
    emit(record)

def removal(template, label, layout, count, entries, breakdown=False):
    with tempfile.TemporaryDirectory(prefix="fixture.", dir=output) as isolate:
        store = Path(isolate) / "store"
        shutil.copytree(template, store)
        server = Server(store, label, breakdown=breakdown)
        sample = None
        try:
            _, port = server.ready()
            client = http.client.HTTPSConnection("127.0.0.1", port, context=context, timeout=60)
            selection = {"kind": "all"}
            if layout == "selected":
                selection = {"kind": "selected", "items": [
                    {"id": eid, "etag": f'"{eid}:1"'} for eid in entries]}
            roots = 1 if layout == "folder" else count
            preview_start = time.monotonic_ns()
            preview = request(client, "POST", "/trash/removals/preview", {"selection": selection})
            preview_ns = time.monotonic_ns() - preview_start
            assert preview["count"] == roots and preview["confirmation"]
            if breakdown and args.sample:
                sample = subprocess.Popen(["sample", str(server.proc.pid), "4", "1",
                                           "-file", str(output / f"{label}.sample.txt")],
                                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                time.sleep(0.10)
            start = time.monotonic_ns()
            state = request(client, "POST", "/trash/removals",
                            {"operation_id": OPERATION, "confirmation": preview["confirmation"]})
            http_ns = time.monotonic_ns() - start
            assert state == {"operation_id": OPERATION, "state": "committed", "count": roots,
                             "cleanup": "pending"}, state
            client.close()
        finally:
            if sample:
                assert sample.wait(timeout=15) == 0
            server.close()
        with connect(store) as db:
            assert db.execute("SELECT removed_count FROM removal WHERE operation_id=?", (OPERATION,)).fetchone() == (roots,)
            assert db.execute("SELECT count(*) FROM removal_member").fetchone() == (count + (layout == "folder"),)
            assert db.execute("SELECT count(*) FROM removal_cleanup").fetchone() == (count,)
        emit({"stage": "removal", "label": label, "shape": layout, "roots": roots, "files": count,
              "preview_http_ns": preview_ns, "preview_fingerprint_ns": server.metric("preview_fingerprint_ns"),
              "http_ns": http_ns, "writer_ns": server.metric("writer_ns"), "fingerprint_ns": server.metric("fingerprint_ns"),
              **({"prepare_ns": int(server.metrics("fingerprint_prepare_ns")[-1][1]["fingerprint_prepare_ns"])}
                 if breakdown else {})})

emit({"UTC": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "load": os.getloadavg(),
      "cpus": os.cpu_count(), "mode": args.mode, "repeats": args.repeats, "idle_seconds": args.idle_seconds})
with tempfile.TemporaryDirectory(prefix="templates.", dir=output) as tmp:
    tmp = Path(tmp)
    if args.mode == "measure":
        removals = [("all", 1), ("selected", 100), ("all", 100), ("all", 1000), ("folder", 1000)]
        histories = [0, 1000, 10_000, 50_000]
    elif args.mode == "review":
        removals = [("folder", 1000, 0), ("folder", 1000, 20_000), ("all", 1, 20_000), ("all", 100, 20_000)]
        histories = [0]
    elif args.mode == "stress":
        removals = [("all", 10_000)]
        histories = [0]
    elif args.mode == "smoke":
        removals = [("selected", 1), ("folder", 2), ("all", 1)]
        histories = [0, 10]
    else:
        removals = [("all", 1000), ("folder", 1000), ("all", 10_000)]
        histories = [50_000]
    history_paths = {}
    for count in histories:
        history_paths[count] = tmp / f"history-{count}"
        seed(history_paths[count], count, "history")
    sql_plans(history_paths[max(histories)])
    templates = {}
    removals = [r if len(r) == 3 else (*r, 0) for r in removals]
    for layout, count, library in removals:
        template = tmp / f"{layout}-{count}-lib{library}"
        entries = seed(template, count, layout, library)
        templates[layout, count, library] = (template, entries)
    for trial in range(args.repeats):
        emit({"trial": trial + 1, "load": os.getloadavg(), "utc": time.strftime("%FT%TZ", time.gmtime())})
        # Rotate cases so size and run order do not coincide.
        for layout, count, library in removals[trial % len(removals):] + removals[:trial % len(removals)]:
            template, entries = templates[layout, count, library]
            removal(template, f"removal-{layout}-{count}-lib{library}-{trial + 1}", layout, count, entries,
                    breakdown=args.mode == "diagnose")
        for count in histories[trial % len(histories):] + histories[:trial % len(histories)]:
            startup(history_paths[count], f"history-{count}-{trial + 1}", diagnostic=args.mode == "diagnose")
        if args.mode in ("measure", "smoke"):
            for count in {min(histories), max(histories)}:
                startup(history_paths[count], f"idle-{count}-{trial + 1}", idle=args.idle_seconds)
    if args.mode == "measure":
        # Independent negative control: persisted non-live data without authorization must block listen.
        store = history_paths[1000]
        with connect(store) as db:
            bad = f"{999_999:032x}"
            db.execute("INSERT INTO blob VALUES(?,?,0,?,'deleted')", (bad, bad[:2], DIGEST))
        server = Server(store, "audit-negative-control")
        server.proc.wait(timeout=60)
        server.close(expected=1)
        [(_, audit)] = server.metrics("audit_ns")
        assert audit["valid"] == "false"
        assert not any('"listener.bind"' in line for _, line in server.events)
        assert any(line == '{"contract_event":"startup.failed","reason":"recovery_required"}'
                   for _, line in server.events)
        emit({"stage": "audit-negative-control", "result": "refused", "status": 1})
emit({"result": "pass", "checked_errors": 0, "utc": time.strftime("%FT%TZ", time.gmtime())})
