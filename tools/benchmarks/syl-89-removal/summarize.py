#!/usr/bin/env python3
from collections import defaultdict
import json
import pathlib
import statistics
import sys

source = pathlib.Path(sys.argv[1])
rows = []
for line in source.read_text().splitlines():
    if line.startswith("{"):
        rows.append(json.loads(line))
assert rows[-1]["result"] == "pass", rows[-1]

def summarize(vals, unit):
    m = statistics.median(vals) / unit
    return f"{m:.3f} [{min(vals)/unit:.3f}..{max(vals)/unit:.3f}]"

def show(title, table, columns, divisor):
    print(title)
    for case, samples in sorted(table.items(), key=lambda pair: str(pair[0])):
        print(f"{str(case):<24} n={len(samples):2d}",
              " | ".join(f"{col} {summarize([sample[col] for sample in samples], divisor)}"
                         for col in columns if col in samples[0]))

where = defaultdict(list)
histories = defaultdict(list)
idle = defaultdict(list)
load = []
for data in rows:
    stage = data.get("stage")
    if stage == "removal":
        where[data["shape"], data["roots"], data["files"], int(data["label"].split("-lib")[1].split("-")[0])].append(data)
    elif stage == "startup":
        kind, count, rep = data["label"].split("-")
        if kind == "history":
            histories[int(count)].append(data)
        elif kind == "idle":
            idle[int(count)].extend(data["idle"])
    elif "load" in data:
        load.append(data["load"][0])
show("Removal ms median [min..max]", where, ("preview_fingerprint_ns", "preview_http_ns", "fingerprint_ns", "writer_ns", "http_ns", "prepare_ns"), 1e6)
show("Startup ms median [min..max]", histories, ("audit_ns", "reconcile_ns", "ready_ns", "child_total_cpu_ns"), 1e6)
show("Idle µs median [min..max]", idle, ("reader_ns", "lookup_ns", "pass_ns"), 1e3)
print("1-min load range:", min(load), max(load))
