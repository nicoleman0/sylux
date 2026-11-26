#!/usr/bin/env python3
import errno
import hashlib
import json
import os
import pty
import select
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
API = "https://api.github.com/repos/nicoleman0/sylux/releases"
DOWNLOAD = "https://github.com/nicoleman0/sylux/releases/download"
STABLE = "v0.1.0"
BETA = "v0.1.0-beta.10"
PAYLOAD = b"""#!/usr/bin/env bash
source "$TEST_INSTALLER_SOURCE"
[[ $# == 1 && "$1" == "$TEST_VERSION" ]]
packages=()
confirm_plan
printf 'Approved %s\\n' "$1"
"""


def asset_url(version):
    return f"{DOWNLOAD}/{version}/install-sylux.sh"


def release(version):
    return {
        "tag_name": version,
        "draft": False,
        "prerelease": version == BETA,
        "assets": [
            {
                "name": "install-sylux.sh",
                "state": "uploaded",
                "size": len(PAYLOAD),
                "browser_download_url": asset_url(version),
                "digest": f"sha256:{hashlib.sha256(PAYLOAD).hexdigest()}",
            }
        ],
    }


def run_pty(command, env, answer):
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(command[0], command, env)
    transcript = b""
    answered = False
    deadline = time.monotonic() + 10
    try:
        while remaining := deadline - time.monotonic():
            if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
                raise AssertionError(f"Timed out waiting for launcher: {transcript!r}")
            try:
                chunk = os.read(fd, 4096)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            if not chunk:
                break
            transcript += chunk
            if not answered and b"Apply this plan? [y/N]" in transcript:
                os.write(fd, answer)
                answered = True
        _, status = os.waitpid(pid, 0)
    except BaseException:
        os.kill(pid, 9)
        os.waitpid(pid, 0)
        raise
    finally:
        os.close(fd)
    assert answered, transcript
    return os.waitstatus_to_exitcode(status), transcript.decode()


def check(label, *, version=STABLE, metadata=None, problem="", answer=None,
          error="", auth=True, requests=2):
    with tempfile.TemporaryDirectory(prefix="sylux-launcher-") as directory:
        fixture = Path(directory)
        (fixture / "assets").mkdir()
        api_url = f"{API}/latest" if version != BETA else f"{API}/tags/{version}"
        (fixture / "payload").write_bytes(PAYLOAD)
        (fixture / "release.json").write_text(
            json.dumps(release(version) if metadata is None else metadata)
        )
        (fixture / "curl").write_text("""#!/usr/bin/env python3
import os
import sys
from pathlib import Path

fixture = Path(os.environ["TEST_FIXTURE"])
api, asset = os.environ["TEST_API"], os.environ["TEST_ASSET"]
problem = os.environ["TEST_PROBLEM"]
args = sys.argv[1:]
assert args[:5] == ["-qfsSL", "--proto", "=https", "--proto-redir", "=https"]
url = args[-1]
with (fixture / "requests").open("a") as output:
    print(url, file=output)
if url == api:
    assert args[-3:-1] == ["-H", "Accept: application/vnd.github+json"]
    if problem in {"api-404", "api-timeout"}:
        sys.exit(22 if problem == "api-404" else 28)
    sys.stdout.buffer.write(b"not JSON" if problem == "api-json" else (fixture / "release.json").read_bytes())
elif url == asset:
    assert args[-3] == "--output"
    path = Path(args[-2])
    assert path.parent == fixture / "assets"
    if problem == "asset-failure":
        path.write_bytes(b"partial")
        sys.exit(22)
    payload = (fixture / "payload").read_bytes()
    if problem == "asset-corrupt":
        payload = b"corrupt"
    if problem == "asset-empty":
        payload = b""
    path.write_bytes(payload)
else:
    sys.exit("Unexpected request")
""")
        (fixture / "sudo").write_text("""#!/bin/sh
set -eu
[ "$#" -eq 3 ] && [ "$1" = bash ] && [ "$3" = "$TEST_VERSION" ]
printf '%s\\n' "$3" >"$TEST_FIXTURE/sudo-called"
exec "$@"
""")
        for stub in ("curl", "sudo"):
            (fixture / stub).chmod(0o700)

        env = dict(
            os.environ,
            PATH=f"{fixture}:{os.environ['PATH']}",
            TMPDIR=str(fixture / "assets"),
            TEST_FIXTURE=str(fixture),
            TEST_API=api_url,
            TEST_ASSET=asset_url(version),
            TEST_PROBLEM=problem,
            TEST_VERSION=version,
            TEST_INSTALLER_SOURCE=str(ROOT / "scripts/install-sylux.sh"),
        )
        args = ["--beta"] if version == BETA else []
        if problem == "arguments":
            args = ["--stable"]
        command = ["sh", "-c", 'launcher=$1; shift; cat "$launcher" | sh -s -- "$@"',
                   "_", str(ROOT / "install.sh"), *args]
        if answer is None:
            result = subprocess.run(command, env=env, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    start_new_session=True, timeout=10, text=True)
            status, output = result.returncode, result.stdout
        else:
            status, output = run_pty(command, env, answer)

        if error:
            assert status != 0 and error in output, (label, status, output)
        else:
            assert status == 0 and f"Approved {version}" in output, (label, status, output)
        assert (fixture / "sudo-called").exists() == auth, (label, output)
        log = fixture / "requests"
        actual = log.read_text().splitlines() if log.exists() else []
        assert actual == [api_url, asset_url(version)][:requests], (label, actual)
        assert not list((fixture / "assets").iterdir()), label
        print(f"Passed: {label}")


check("promoted stable preserves interactive approval", answer=b"y\n")
check("later stable preserves interactive approval", version="v0.1.1", answer=b"y\n")
check("interactive decline propagates", answer=b"n\n", error="Plan declined.")
check("explicit beta.10 preserves interactive approval", version=BETA, answer=b"y\n")
check("missing terminal", auth=False, error="interactive terminal")
check("unknown arguments", problem="arguments", auth=False, requests=0, error="pass --beta")
for failure in ("api-404", "api-timeout"):
    check(failure, problem=failure, auth=False, requests=1, error="Could not confirm")
check("invalid response", problem="api-json", auth=False, requests=1, error="expected stable installer")
for fields in ({"tag_name": "v0.0.1"}, {"tag_name": BETA}, {"tag_name": "v00.1.0"}, {"tag_name": "v0.1.0-rc.1"}, {"tag_name": "v0.1"}, {"draft": True},
               {"prerelease": True}, {"assets": []},
               {"assets": release(STABLE)["assets"] * 2}):
    check(str(fields), metadata=release(STABLE) | fields, auth=False, requests=1,
          error="expected stable installer")
for fields in ({"state": "starter"}, {"size": 0}, {"digest": None},
               {"browser_download_url": asset_url(BETA)}):
    metadata = release(STABLE)
    metadata["assets"][0].update(fields)
    check(str(fields), metadata=metadata, auth=False, requests=1, error="expected stable installer")
check("stable asset download failure", problem="asset-failure", auth=False, error="Could not download")
check("stable empty asset", problem="asset-empty", auth=False, error="Could not download")
check("stable corrupt asset", problem="asset-corrupt", auth=False, error="checksum did not match")
check("explicit beta missing", version=BETA, problem="api-404", auth=False, requests=1,
      error="Could not confirm")
check("explicit beta marked stable", version=BETA, metadata=release(BETA) | {"prerelease": False},
      auth=False, requests=1, error="expected beta installer")
