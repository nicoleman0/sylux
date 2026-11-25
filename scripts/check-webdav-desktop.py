"""Drive Thunar on a disposable Xvfb / D-Bus session. Requires runner-local sudo for the test CA."""

import base64
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import subprocess
import sys
import tempfile
import time

import pyatspi


ACCOUNT = "desktop-fixture"
SEED = b"Seeded fixture download bytes\0\xff"
UPLOAD = b"Thunar fixture upload bytes\0\xfe"
ENV = {**os.environ, "LC_ALL": "C", "GTK_A11Y": "1", "NO_AT_BRIDGE": "0"}


def record(event, **fields):
    print(json.dumps({"event": event, **fields}), flush=True)


def run(*args, quiet=True):
    return subprocess.run(
        args, env=ENV, check=True, timeout=15,
        stdout=subprocess.DEVNULL if quiet else subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )


@contextlib.contextmanager
def process(*args, env=None):
    child = subprocess.Popen(args, env=env or ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        yield child
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)


def eventually(label, operation, seconds=12):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = operation()
        if result:
            return result
        time.sleep(0.15)
    raise RuntimeError(f"Timed out: {label}")


def create_tls(directory):
    cert = directory / "ca.crt"
    ca_key = directory / "ca.key"
    key = directory / "leaf.key"
    csr = directory / "leaf.csr"
    leaf = directory / "leaf.crt"
    extensions = directory / "leaf.ext"
    extensions.write_text("subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n")
    run("openssl", "req", "-new", "-x509", "-nodes", "-days", "2", "-newkey", "rsa:2048",
        "-addext", "basicConstraints=critical,CA:TRUE",
        "-subj", "/CN=Sylux disposable desktop CA", "-keyout", ca_key, "-out", cert)
    run("openssl", "req", "-new", "-nodes", "-newkey", "rsa:2048",
        "-subj", "/CN=127.0.0.1", "-keyout", key, "-out", csr)
    run("openssl", "x509", "-req", "-in", csr, "-CA", cert, "-CAkey", ca_key, "-CAcreateserial",
        "-days", "2", "-extfile", extensions, "-out", leaf)
    key.chmod(0o600)
    return cert, leaf, key


@contextlib.contextmanager
def trusted_ca(cert):
    target = Path("/usr/local/share/ca-certificates") / f"sylux-desktop-{secrets.token_hex(8)}.crt"
    try:
        run("sudo", "-n", "install", "-m", "0644", cert, target)
        run("sudo", "-n", "update-ca-certificates")
        yield
    finally:
        run("sudo", "-n", "rm", "-f", target)
        run("sudo", "-n", "update-ca-certificates", "--fresh")


class Fixture:
    def __init__(self, port, cert):
        self.port = port
        self.ssl_context = ssl.create_default_context(cafile=str(cert))
        self.origin = f"https://127.0.0.1:{port}"
        self.mount = f"davs://{ACCOUNT}@127.0.0.1:{port}/dav/Files/"
        self.cookies = ""
        self.csrf = ""
        self.dav = ""

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPSConnection("127.0.0.1", self.port, context=self.ssl_context, timeout=3)
        try:
            conn.request(method, path, body, headers or {})
            response = conn.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            conn.close()

    def expect(self, label, method, path, status, body=None, headers=None):
        actual, received_headers, received = self.request(method, path, body, headers)
        record("http", step=label, method=method, status=actual)
        if actual != status:
            raise RuntimeError(f"{label}: expected HTTP {status}")
        return received_headers, received

    def bootstrap(self, authorization):
        password = secrets.token_urlsafe(28)
        login = json.dumps({"handle": ACCOUNT, "password": password}).encode()
        origin = {"Origin": self.origin, "Content-Type": "application/json"}
        self.expect("setup", "POST", "/setup/admin", 201, login,
                    {**origin, "X-Setup-Authorization": authorization})
        headers, body = self.expect("anonymous-csrf", "GET", "/auth/csrf", 200)
        anonymous_cookie = get_header(headers, "set-cookie").split(";", 1)[0]
        headers, body = self.expect("login", "POST", "/auth/local", 200, login, {
            **origin, "Cookie": anonymous_cookie, "X-CSRF-Token": json.loads(body)["csrf_token"],
        })
        self.cookies = get_header(headers, "set-cookie").split(";", 1)[0]
        self.csrf = json.loads(body)["csrf_token"]
        _, body = self.expect("create-app-password", "POST", "/me/webdav", 201,
                              b'{"name":"Thunar fixture","read_only":false}', self.browser_headers())
        issued = json.loads(body)
        app_password = issued["password"]
        self.dav = "Basic " + base64.b64encode(f"{ACCOUNT}:{app_password}".encode()).decode()
        return app_password, issued["credential"]["id"]

    def browser_headers(self):
        return {"Origin": self.origin, "Cookie": self.cookies, "X-CSRF-Token": self.csrf,
                "Content-Type": "application/json"}

    def dav_expect(self, label, method, path, status, body=None, **headers):
        return self.expect(label, method, path, status, body, {"Authorization": self.dav, **headers})

    def wait_for_bytes(self, label, target, expected):
        def test():
            status, _, payload = self.request("GET", target, headers={"Authorization": self.dav})
            return status == 200 and payload == expected
        eventually(label, test)
        record("http", step=label, method="GET", status=200, sha256=hashlib.sha256(expected).hexdigest())


def get_header(headers, name):
    return next(value for header, value in headers.items() if header.lower() == name)


def thunar_controls():
    desktop = pyatspi.Registry.getDesktop(0)
    for app in desktop:
        if app and app.name.lower() == "thunar":
            stack = [app]
            while stack:
                control = stack.pop()
                if control:
                    yield control
                    stack.extend(control)


def control_named(name, role=None):
    return next((item for item in thunar_controls()
                 if (item.name == name or (role == pyatspi.ROLE_FRAME and item.name.startswith(name + " - ")))
                 and (role is None or item.getRole() == role)
                 and item.getState().contains(pyatspi.STATE_SHOWING)), None)


def key(*keys):
    run("xdotool", "key", "--clearmodifiers", *keys)


def type_text(value):
    run("xdotool", "type", "--clearmodifiers", "--", value)


def navigate(location):
    key("ctrl+l", "ctrl+a")
    type_text(location)
    key("Return")


def select(name):
    key("ctrl+s")
    eventually("Thunar select by pattern", lambda: control_named("Select by Pattern"))
    key("ctrl+a")
    type_text(name)
    key("Return")
    eventually("Thunar selection dialog closed", lambda: not control_named("Select by Pattern"))


def activate(control):
    action = control.queryAction()
    if not action.doAction(0):
        raise RuntimeError("Thunar control could not be activated")


def verify_digest(label, actual, expected):
    if actual != expected:
        raise RuntimeError(f"{label}: byte mismatch")
    record("bytes", step=label, length=len(actual), sha256=hashlib.sha256(actual).hexdigest())


def desktop(fixture, directory, app_password):
    downloads = directory / "download"
    uploads = directory / "upload"
    downloads.mkdir()
    uploads.mkdir()
    (uploads / "upload.bin").write_bytes(UPLOAD)

    with process("openbox"), process("thunar", "--new-window", str(downloads)):
        eventually("Thunar window", lambda: control_named(downloads.name, pyatspi.ROLE_FRAME))
        navigate(fixture.mount)
        def password_entry():
            return next((item for item in thunar_controls() if item.getRole() == pyatspi.ROLE_PASSWORD_TEXT
                         and item.getState().contains(pyatspi.STATE_SHOWING)), None)
        password = eventually("Thunar app password prompt", password_entry)
        forget = control_named("Forget password immediately", pyatspi.ROLE_RADIO_BUTTON)
        if forget is not None:
            activate(forget)
        if not password.queryEditableText().setTextContents(app_password):
            raise RuntimeError("Thunar password field did not accept input")
        connect = eventually("Thunar connect button", lambda: control_named("Connect", pyatspi.ROLE_PUSH_BUTTON))
        activate(connect)
        eventually("Thunar list contains seeded file", lambda: control_named("seed.bin"))
        record("gui", step="connect-list", action="location bar, password prompt, list row")

        select("seed.bin")
        key("ctrl+c")
        navigate(str(downloads))
        eventually("Thunar local download folder", lambda: control_named(downloads.name, pyatspi.ROLE_FRAME))
        key("ctrl+v")
        download = downloads / "seed.bin"
        eventually("Thunar downloaded file", lambda: download.is_file() and download.stat().st_size == len(SEED))
        record("gui", step="download", action="select, copy, location bar, paste")
        verify_digest("download", download.read_bytes(), SEED)

        navigate(str(uploads))
        eventually("Thunar local upload folder", lambda: control_named("upload.bin"))
        select("upload.bin")
        key("ctrl+c")
        navigate(fixture.mount)
        eventually("Thunar return to Files", lambda: control_named("seed.bin"))
        key("ctrl+v")
        fixture.wait_for_bytes("uploaded", "/dav/Files/upload.bin", UPLOAD)
        record("gui", step="upload", action="select, copy, location bar, paste")

        eventually("Thunar uploaded file row", lambda: control_named("upload.bin"))
        select("upload.bin")
        key("F2")
        eventually("Thunar rename dialog", lambda: control_named("Rename", pyatspi.ROLE_PUSH_BUTTON))
        key("ctrl+a")
        type_text("renamed.bin")
        key("Return")
        fixture.wait_for_bytes("renamed", "/dav/Files/renamed.bin", UPLOAD)
        fixture.dav_expect("renamed-source-gone", "GET", "/dav/Files/upload.bin", 404)
        record("gui", step="rename", action="select, F2, enter name")

        eventually("Thunar renamed file row", lambda: control_named("renamed.bin"))
        select("renamed.bin")
        key("ctrl+x")
        navigate(fixture.mount + "Destination/")
        eventually("Thunar destination folder", lambda: control_named("Destination", pyatspi.ROLE_FRAME))
        key("ctrl+v")
        target = "/dav/Files/Destination/renamed.bin"
        fixture.wait_for_bytes("moved", target, UPLOAD)
        fixture.dav_expect("moved-source-gone", "GET", "/dav/Files/renamed.bin", 404)
        record("gui", step="move", action="select, cut, location bar, paste")

        eventually("Thunar moved file row", lambda: control_named("renamed.bin"))
        select("renamed.bin")
        key("shift+Delete")
        delete = eventually("Thunar delete confirmation", lambda: control_named("Delete", pyatspi.ROLE_PUSH_BUTTON))
        activate(delete)
        def deleted():
            return fixture.request("GET", target, headers={"Authorization": fixture.dav})[0] == 404
        eventually("Thunar deleted file", deleted)
        record("http", step="deleted-source-gone", method="GET", status=404)
        record("gui", step="delete", action="select, Shift+Delete, confirm")
        _, body = fixture.expect("sylux-trash", "GET", "/trash", 200, headers=fixture.browser_headers())
        if "renamed.bin" not in {item["name"] for item in json.loads(body)["items"]}:
            raise RuntimeError("Thunar delete did not reach Sylux Trash")


def main(binary):
    if not (os.environ.get("DISPLAY") and os.environ.get("DBUS_SESSION_BUS_ADDRESS")):
        raise RuntimeError("Run through xvfb-run and dbus-run-session")
    versions = run("dpkg-query", "-W", "-f=${Package} ${Version}\\n",
                   "thunar", "gvfs-backends", "dbus-x11", "xvfb", "xdotool", quiet=False)
    record("versions", packages=versions.stdout.decode().splitlines())
    with tempfile.TemporaryDirectory(prefix="sylux-desktop-") as temporary:
        directory = Path(temporary)
        root = directory / "product"
        root.mkdir(mode=0o700)
        client_home = directory / "client-home"
        client_home.mkdir(mode=0o700)
        ENV.update(HOME=str(client_home), XDG_CONFIG_HOME=str(client_home / "config"),
                   XDG_CACHE_HOME=str(client_home / "cache"), XDG_DATA_HOME=str(client_home / "data"))
        run("dbus-update-activation-environment", "HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME",
            "XDG_DATA_HOME", "DISPLAY", "XAUTHORITY", "GTK_A11Y", "NO_AT_BRIDGE")
        cert, leaf, key_file = create_tls(directory)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        fixture = Fixture(port, cert)
        run(binary, "init", "--root", root)
        with trusted_ca(cert), process(binary, "serve", "--root", root, "--webdav",
                                       "--listen", f"127.0.0.1:{port}", "--tls-key", key_file,
                                       "--tls-cert", leaf, "--public-origin", fixture.origin):
            def ready():
                try:
                    return fixture.request("GET", "/readyz")[0] == 200
                except OSError:
                    return False
            eventually("Sylux ready over loopback TLS", ready)
            password, credential_id = fixture.bootstrap((root / "secrets/setup.authorization").read_text())
            fixture.dav_expect("seed-file", "PUT", "/dav/Files/seed.bin", 201, SEED)
            fixture.dav_expect("seed-destination", "MKCOL", "/dav/Files/Destination/", 201)
            fixture.dav_expect("before-upload-absent", "GET", "/dav/Files/upload.bin", 404)
            desktop(fixture, directory, password)
            fixture.expect("revoke-client", "DELETE", f"/me/webdav/{credential_id}", 204, headers=fixture.browser_headers())
            fixture.dav_expect("revoked-list", "PROPFIND", "/dav/Files/", 401, Depth="1")
            record("result", status="passed")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: xvfb-run -a dbus-run-session -- python3 scripts/check-webdav-desktop.py PATH_TO_SYLUX")
    main(Path(sys.argv[1]).resolve(strict=True))
