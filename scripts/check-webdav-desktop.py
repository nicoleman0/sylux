"""Drive Thunar on a disposable Xvfb / D-Bus session. Requires runner-local sudo for the test CA."""

import base64
from collections import Counter
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import socket
import ssl
import subprocess
import sys
import tempfile
import time

import pyatspi
from gi.repository import GLib


ACCOUNT = "desktop-fixture"
SEED = b"Seeded fixture download bytes\0\xff"
UPLOAD = b"Thunar fixture upload bytes\0\xfe"
ENV = {**os.environ, "LC_ALL": "C", "GDK_BACKEND": "x11", "NO_AT_BRIDGE": "0",
       "GIO_USE_VFS": "gvfs"}


def record(event, **fields):
    print(json.dumps({"event": event, **fields}), flush=True)


def run(*args, quiet=True, input_bytes=None):
    return subprocess.run(
        args, env=ENV, check=True, timeout=15,
        input=input_bytes,
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


def eventually(label, operation, seconds=12, on_timeout=None):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = operation()
        if result:
            return result
        time.sleep(0.15)
    if on_timeout is not None:
        on_timeout(label)
    raise RuntimeError(f"Timed out: {label}")


def wait_ready(fixture, child, seconds=12):
    end = time.monotonic() + seconds
    last = "no response"
    while time.monotonic() < end:
        if child.poll() is not None:
            raise RuntimeError(f"Sylux exited before readiness (exit code {child.returncode}; last: {last})")
        try:
            status = fixture.request("GET", "/readyz")[0]
            if status == 204:
                return
            last = f"HTTP {status}"
        except ssl.SSLError:
            last = "TLS error"
        except (OSError, http.client.HTTPException):
            last = "connection or HTTP error"
        time.sleep(0.15)
    raise RuntimeError(f"Timed out: Sylux ready over loopback TLS (last: {last})")


def create_tls(directory):
    cert = directory / "ca.crt"
    ca_key = directory / "ca.key"
    key = directory / "leaf.key"
    csr = directory / "leaf.csr"
    leaf = directory / "leaf.crt"
    extensions = directory / "leaf.ext"
    extensions.write_text(
        "subjectAltName=IP:127.0.0.1\n"
        "basicConstraints=critical,CA:FALSE\n"
        "extendedKeyUsage=serverAuth\n"
        "subjectKeyIdentifier=hash\n"
        "authorityKeyIdentifier=keyid:always\n"
    )
    run("openssl", "req", "-new", "-x509", "-nodes", "-days", "2", "-newkey", "rsa:2048",
        "-addext", "basicConstraints=critical,CA:TRUE",
        "-addext", "keyUsage=critical,keyCertSign,cRLSign",
        "-addext", "subjectKeyIdentifier=hash",
        "-addext", "authorityKeyIdentifier=keyid:always",
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

    def wait_for_bytes(self, label, target, expected, on_timeout=None):
        def test():
            status, headers, payload = self.request("GET", target, headers={"Authorization": self.dav})
            if status == 200 and payload == expected:
                return get_header(headers, "etag")
        etag = eventually(label, test, on_timeout=on_timeout)
        record("http", step=label, method="GET", status=200, sha256=hashlib.sha256(expected).hexdigest())
        return etag


def get_header(headers, name):
    return next(value for header, value in headers.items() if header.lower() == name)


def x11(*args):
    try:
        result = subprocess.run(("xdotool", *args), env=ENV, timeout=2,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        return ""
    return result.stdout.decode(errors="replace").strip() if result.returncode == 0 else ""


def thunar_windows(folder=None):
    # xdotool search accepts one positional regex; inspect titles after discovery.
    windows = x11("search", "--all", "--onlyvisible", "--class", "^Thunar$").splitlines()
    if folder is None:
        return windows
    expected = f"{folder} - Thunar"
    return [window for window in windows if x11("getwindowname", window) == expected]


def enable_accessibility():
    run("gdbus", "call", "--session", "--dest", "org.a11y.Bus", "--object-path", "/org/a11y/bus",
        "--method", "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", "IsEnabled", "<true>")
    # Start the registry before GTK tries to register its accessible objects.
    registry = pyatspi.Registry.getDesktop(0)
    if registry is None:
        raise RuntimeError("AT-SPI registry did not start")
    return registry


def controls(root):
    stack = [root]
    while stack:
        control = stack.pop()
        if control is not None:
            yield control
            stack.extend(control)


class Thunar:
    def __init__(self, registry):
        self.registry = registry
        self.window = None
        self.pid = None
        self.child = None

    def app(self):
        if self.pid is None:
            return None
        # Dispatch registration and cache updates between polls.
        context = GLib.MainContext.default()
        for _ in range(16):
            if not context.iteration(False):
                break
        for app in self.registry:
            if app is not None:
                try:
                    if app.get_process_id() == self.pid:
                        return app
                except GLib.Error:
                    continue
        return None

    def wait(self, stage, operation):
        def probe():
            if self.child is not None and self.child.poll() is not None:
                raise RuntimeError(f"Thunar exited before {stage} (exit code {self.child.returncode})")
            try:
                return operation()
            except GLib.Error:
                # A control can disappear while GTK replaces a view or closes a dialog.
                return None
        return eventually(stage, probe, on_timeout=self.diagnose)

    def diagnose(self, stage):
        counts = Counter()
        roles = {pyatspi.ROLE_FRAME: "frames", pyatspi.ROLE_DIALOG: "dialogs",
                 pyatspi.ROLE_ALERT: "alerts", pyatspi.ROLE_PASSWORD_TEXT: "password_entries",
                 pyatspi.ROLE_TABLE: "tables", pyatspi.ROLE_TABLE_CELL: "table_cells"}
        registered = False
        try:
            app = self.app()
            registered = app is not None
            for index, item in enumerate(controls(app)):
                if index == 400:
                    break
                role = roles.get(item.getRole())
                if role and item.getState().contains(pyatspi.STATE_SHOWING):
                    counts[role] += 1
        except GLib.Error:
            counts["accessibility_error"] = 1
        record("gui-timeout", step=stage, x11_windows=len(thunar_windows()),
               thunar_registered=registered, showing=dict(counts))

    def attach(self, folder):
        def find_window():
            windows = thunar_windows(folder)
            if len(windows) == 1:
                pid = x11("getwindowpid", windows[0])
                if pid.isdecimal():
                    return windows[0], int(pid)

        self.window, self.pid = self.wait("Thunar named X11 window", find_window)
        self.wait("Thunar X11 focus", self.focus)
        record("gui", step="x11-window")
        self.wait("Thunar AT-SPI registration", self.app)
        record("gui", step="accessibility-registration")

    def focus(self):
        if x11("getactivewindow") == self.window:
            return True
        x11("windowactivate", self.window)
        return False

    def at_folder(self, folder):
        return self.window in thunar_windows(folder) and x11("getactivewindow") == self.window

    def control(self, role, name=None):
        return next((item for item in controls(self.app())
                     if item.getRole() == role and item.getState().contains(pyatspi.STATE_SHOWING)
                     and (name is None or item.name == name)), None)

    def row(self, name):
        return self.control(pyatspi.ROLE_TABLE_CELL, name)

    def select(self, name):
        key("ctrl+s")
        self.wait("Thunar select by pattern", lambda: self.control(pyatspi.ROLE_DIALOG, "Select by Pattern"))
        key("ctrl+a")
        type_text(name)
        key("Return")
        self.wait("Thunar selection dialog closed", lambda: x11("getactivewindow") == self.window
                  and self.control(pyatspi.ROLE_DIALOG, "Select by Pattern") is None)


def key(*keys):
    run("xdotool", "key", "--clearmodifiers", *keys)


def type_text(value):
    run("xdotool", "type", "--clearmodifiers", "--file", "-", input_bytes=value.encode())


def navigate(location):
    key("ctrl+l", "ctrl+a")
    type_text(location)
    key("Return")


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

    thunar = Thunar(enable_accessibility())
    with contextlib.ExitStack() as processes:
        processes.enter_context(process("openbox"))
        thunar.wait("Openbox X11 desktop", lambda: x11("get_desktop"))
        thunar.child = processes.enter_context(process("thunar", str(downloads)))
        thunar.attach(downloads.name)
        key("ctrl+2")
        thunar.wait("Thunar detailed list view", lambda: thunar.control(pyatspi.ROLE_TABLE))
        navigate(fixture.mount)
        password = thunar.wait("Thunar app password prompt", lambda: thunar.control(pyatspi.ROLE_PASSWORD_TEXT))
        forget = thunar.control(pyatspi.ROLE_RADIO_BUTTON, "Forget password immediately")
        if forget is not None:
            activate(forget)
        if not password.queryEditableText().setTextContents(app_password):
            raise RuntimeError("Thunar password field did not accept input")
        connect = thunar.wait("Thunar connect button", lambda: thunar.control(pyatspi.ROLE_PUSH_BUTTON, "Connect"))
        activate(connect)
        thunar.wait("Thunar list contains seeded file", lambda: thunar.row("seed.bin"))
        record("gui", step="connect-list", action="location bar, password prompt, list row")

        thunar.select("seed.bin")
        key("ctrl+c")
        navigate(str(downloads))
        thunar.wait("Thunar local download folder", lambda: thunar.at_folder(downloads.name))
        key("ctrl+v")
        download = downloads / "seed.bin"
        thunar.wait("Thunar downloaded file", lambda: download.is_file() and download.stat().st_size == len(SEED))
        record("gui", step="download", action="select, copy, location bar, paste")
        verify_digest("download", download.read_bytes(), SEED)

        navigate(str(uploads))
        thunar.wait("Thunar local upload folder", lambda: thunar.row("upload.bin"))
        thunar.select("upload.bin")
        key("ctrl+c")
        navigate(fixture.mount)
        thunar.wait("Thunar return to Files", lambda: thunar.row("seed.bin"))
        key("ctrl+v")
        uploaded_etag = fixture.wait_for_bytes("uploaded", "/dav/Files/upload.bin", UPLOAD, thunar.diagnose)
        record("gui", step="upload", action="select, copy, location bar, paste")

        thunar.wait("Thunar uploaded file row", lambda: thunar.row("upload.bin"))
        thunar.select("upload.bin")
        key("F2")
        thunar.wait("Thunar rename dialog", lambda: thunar.control(pyatspi.ROLE_PUSH_BUTTON, "Rename"))
        key("ctrl+a")
        type_text("renamed.bin")
        key("Return")
        if fixture.wait_for_bytes("renamed", "/dav/Files/renamed.bin", UPLOAD, thunar.diagnose) != uploaded_etag:
            raise RuntimeError("Thunar rename changed the file revision")
        fixture.dav_expect("renamed-source-gone", "GET", "/dav/Files/upload.bin", 404)
        record("gui", step="rename", action="select, F2, enter name")

        thunar.wait("Thunar renamed file row", lambda: thunar.row("renamed.bin"))
        thunar.select("renamed.bin")
        key("ctrl+x")
        navigate(fixture.mount + "Destination/")
        thunar.wait("Thunar destination folder", lambda: thunar.at_folder("Destination"))
        key("ctrl+v")
        target = "/dav/Files/Destination/renamed.bin"
        if fixture.wait_for_bytes("moved", target, UPLOAD, thunar.diagnose) != uploaded_etag:
            raise RuntimeError("Thunar move changed the file revision")
        fixture.dav_expect("moved-source-gone", "GET", "/dav/Files/renamed.bin", 404)
        record("gui", step="move", action="select, cut, location bar, paste")

        _, body = fixture.expect("trash-before-delete", "GET", "/trash", 200, headers=fixture.browser_headers())
        if json.loads(body)["items"]:
            raise RuntimeError("Thunar rename or move put an item in Sylux Trash")

        thunar.wait("Thunar moved file row", lambda: thunar.row("renamed.bin"))
        thunar.select("renamed.bin")
        key("shift+Delete")
        delete = thunar.wait("Thunar delete confirmation", lambda: thunar.control(pyatspi.ROLE_PUSH_BUTTON, "Delete"))
        activate(delete)
        def deleted():
            return fixture.request("GET", target, headers={"Authorization": fixture.dav})[0] == 404
        thunar.wait("Thunar deleted file", deleted)
        record("http", step="deleted-source-gone", method="GET", status=404)
        record("gui", step="delete", action="select, Shift+Delete, confirm")
        _, body = fixture.expect("sylux-trash", "GET", "/trash", 200, headers=fixture.browser_headers())
        if "renamed.bin" not in {item["name"] for item in json.loads(body)["items"]}:
            raise RuntimeError("Thunar delete did not reach Sylux Trash")


def main(binary):
    if not (os.environ.get("DISPLAY") and os.environ.get("DBUS_SESSION_BUS_ADDRESS")):
        raise RuntimeError("Run through xvfb-run and dbus-run-session")
    versions = run("dpkg-query", "-W", "-f=${Package} ${Version}\\n",
                   "thunar", "gvfs", "gvfs-backends", "glib-networking", "libgtk-3-0t64",
                   "at-spi2-core", "dbus-x11", "openbox", "xvfb", "xdotool", quiet=False)
    record("versions", packages=versions.stdout.decode().splitlines())
    with tempfile.TemporaryDirectory(prefix="sylux-desktop-") as temporary:
        directory = Path(temporary)
        root = directory / "product"
        root.mkdir(mode=0o700)
        client_home = directory / "client-home"
        client_home.mkdir(mode=0o700)
        runtime = directory / "runtime"
        runtime.mkdir(mode=0o700)
        os.environ["XDG_RUNTIME_DIR"] = str(runtime)
        ENV.update(HOME=str(client_home), XDG_CONFIG_HOME=str(client_home / "config"),
                   XDG_CACHE_HOME=str(client_home / "cache"), XDG_DATA_HOME=str(client_home / "data"),
                   XDG_RUNTIME_DIR=str(runtime))
        run("dbus-update-activation-environment", "HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME",
            "XDG_DATA_HOME", "XDG_RUNTIME_DIR", "DISPLAY", "XAUTHORITY", "GDK_BACKEND",
            "NO_AT_BRIDGE", "GIO_USE_VFS")
        cert, leaf, key_file = create_tls(directory)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        fixture = Fixture(port, cert)
        run(binary, "init", "--root", root)
        with trusted_ca(cert), process(binary, "serve", "--root", root, "--webdav",
                                       "--listen", f"127.0.0.1:{port}", "--tls-key", key_file,
                                       "--tls-cert", leaf, "--public-origin", fixture.origin) as server:
            wait_ready(fixture, server)
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
