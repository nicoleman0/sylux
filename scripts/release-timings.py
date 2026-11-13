#!/usr/bin/env python3
"""Report GitHub Actions queue, job and step timings using the authenticated gh CLI."""

import argparse
import datetime
import json
import subprocess


def api(path):
    result = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", path],
        check=True, capture_output=True, text=True,
    )
    return json.loads(result.stdout)


def instant(value):
    return datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))


def seconds(start, end):
    return round((instant(end) - instant(start)).total_seconds())


def report(repo, run_id):
    path = f"repos/{repo}/actions/runs/{run_id}"
    run = api(path)[0]
    if run["status"] != "completed":
        raise ValueError(f"Run {run_id} is not complete")
    if run["run_attempt"] != 1:
        raise ValueError(f"Run {run_id} was rerun; use a new dispatch to measure queue time")
    jobs = [job for page in api(path + "/jobs?filter=latest&per_page=100") for job in page["jobs"]]
    active = [job for job in jobs if job["conclusion"] != "skipped"]
    if not active:
        raise ValueError(f"Run {run_id} has no executed jobs")
    start = min(job["started_at"] for job in active)
    end = max(job["completed_at"] for job in active)
    print(f"## [{run_id}]({run['html_url']})")
    print(f"\nSource: `{run['head_sha']}`. Attempt: {run['run_attempt']}. Result: {run['conclusion']}.")
    print(f"Initial queue: {seconds(run['created_at'], start)}s. End to end: {seconds(run['created_at'], end)}s.")
    print("\n| Job / step | Seconds | Result |\n| --- | ---: | --- |")
    for job in active:
        print(f"| **{job['name']}** | **{seconds(job['started_at'], job['completed_at'])}** | {job['conclusion']} |")
        for step in job["steps"]:
            if step["status"] == "completed" and step.get("started_at") and step.get("completed_at"):
                print(f"| {step['name']} | {seconds(step['started_at'], step['completed_at'])} | {step['conclusion']} |")
    print()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_ids", nargs="+", type=int)
    parser.add_argument("--repo", default="nicoleman0/sylux")
    args = parser.parse_args()
    for run_id in args.run_ids:
        if run_id <= 0:
            parser.error("Run IDs must be positive")
        report(args.repo, run_id)


if __name__ == "__main__":
    main()
