"""Exercise the browser's published HTTP flows with disposable data over trusted TLS."""

import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import ssl
import sys
import time

ORIGIN = "https://127.0.0.1:3000"
ACCOUNTS = {"fixture-admin": "administrator", "fixture-member": "user"}
PAYLOADS = {
    "notes.txt": b"Disposable upgrade rehearsal notes.\n" * 19,
    "photos.bin": bytes(range(256)) * 3073,
    "member.bin": bytes(range(255, -1, -1)) * 23,
    "retained.txt": b"Disposable file retained in Trash.\n" * 31,
}


class CheckFailed(Exception):
    pass


def check(condition, message):
    if not condition:
        raise CheckFailed(message)


def evidence(message):
    print(message, flush=True)
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as summary:
        print(f"- {message}", file=summary)


class Client:
    def __init__(self, trust):
        self.context = ssl.create_default_context(cafile=str(trust))
        self.session = {}

    def request(self, label, method, path, status, body=None, headers=None):
        outgoing = {"Origin": ORIGIN, **self.session, **(headers or {})}
        if isinstance(body, dict):
            outgoing["Content-Type"] = "application/json"
            body = json.dumps(body).encode()
        connection = http.client.HTTPSConnection("127.0.0.1", 3000, context=self.context, timeout=5)
        try:
            connection.request(method, path, body, outgoing)
            response = connection.getresponse()
            check(response.status == status, f"{label}: expected HTTP {status}, received {response.status}.")
            return response.headers, response.read()
        except (OSError, http.client.HTTPException) as error:
            raise CheckFailed(f"{label}: TLS or HTTP transport failed ({type(error).__name__}).") from None
        finally:
            connection.close()

    def data(self, *args, **kwargs):
        return json.loads(self.request(*args, **kwargs)[1])

    def sign_in(self, credentials):
        headers, body = self.request("anonymous CSRF", "GET", "/auth/csrf", 200)
        self.session = {
            "Cookie": headers["Set-Cookie"].split(";", 1)[0],
            "X-CSRF-Token": json.loads(body)["csrf_token"],
        }
        headers, body = self.request("local sign-in", "POST", "/auth/local", 200, credentials)
        self.session = {
            "Cookie": headers["Set-Cookie"].split(";", 1)[0],
            "X-CSRF-Token": json.loads(body)["csrf_token"],
        }
        return self.data("current signed-in account", "GET", "/me", 200)


def items(client, label, path):
    page = client.data(label, "GET", path, 200)
    check(not page.get("has_more") and not page.get("next_cursor"), "Fixture inventory exceeded one page.")
    return sorted(page["items"], key=lambda item: item["name"])


def listing(client, folder=None):
    target = "/folders/children" if folder is None else f"/folders/{folder}/children"
    return items(client, "private folder listing", target)


def root(client):
    return client.data("private root", "GET", "/folders/children", 200)["root"]


def upload(client, parent, name):
    if parent is None:
        parent = root(client)
    payload = PAYLOADS[name]
    body = {"parent_id": parent["id"], "parent_etag": parent["etag"], "name": name, "length": len(payload)}
    ticket = client.data("upload ticket", "POST", "/mutation-tickets", 201, dict(body, scope="upload_create"))
    headers = {"If-None-Match": "*", "X-Mutation-ID": ticket["ticket"]}
    client.request("upload admission", "POST", "/uploads", 201, body, headers)
    path = f"/uploads/{ticket['mutation_id']}"
    staged = client.data("upload content", "PUT", f"{path}/content", 200, payload,
                         {**headers, "Content-Type": "application/octet-stream"})
    check(staged["sha256"] == digest(payload), "Server staging digest differed from original bytes.")
    return client.data("upload commit", "POST", f"{path}/commit", 201,
                       {"sha256": staged["sha256"], "parent_etag": parent["etag"]}, headers)


def digest(payload):
    return hashlib.sha256(payload).hexdigest()


def download(client, label, path, name):
    headers, received = client.request(label, "GET", path, 200)
    expected = PAYLOADS[name]
    check(int(headers["Content-Length"]) == len(expected) and len(received) == len(expected)
          and digest(received) == digest(expected) and received == expected,
          f"{label}: download size, SHA-256 or bytes differed.")
    return {"size": len(received), "sha256": digest(received)}


def sign_in_both(trust, credentials):
    sessions, identities = {}, {}
    for handle, role in ACCOUNTS.items():
        client = Client(trust)
        me = client.sign_in(credentials[handle])
        check(me["handle"] == handle and me["role"] == role, "Signed-in account identity or role differed.")
        sessions[role] = client
        identities[role] = {field: me[field] for field in ("account_id", "handle", "role")}
    check(identities["administrator"]["account_id"] != identities["user"]["account_id"],
          "Expected two distinct accounts.")
    return sessions["administrator"], sessions["user"], identities


def link_path(link):
    url = link["url"]
    check(re.fullmatch(re.escape(ORIGIN) + r"/share/[A-Za-z0-9_-]+", url), "Unexpected download-link origin or path.")
    return url.removeprefix(ORIGIN)


def seed(trust, authorization):
    anonymous = Client(trust)
    credentials = {handle: {"handle": handle, "password": secrets.token_urlsafe(30)} for handle in ACCOUNTS}
    owner, member = credentials.values()
    anonymous.request("administrator setup", "POST", "/setup/admin", 201, owner,
                      {"X-Setup-Authorization": authorization})
    admin = Client(trust)
    admin.sign_in(owner)
    admin.request("member creation", "POST", "/accounts", 204, member)
    admin, user, _ = sign_in_both(trust, credentials)
    notes = upload(admin, None, "notes.txt")
    admin_root = root(admin)
    folder = admin.data("nested folder creation", "POST", f"/folders/{admin_root['id']}/children", 201,
                        {"name": "Photos"})
    upload(admin, folder, "photos.bin")
    upload(user, None, "member.bin")
    upload(admin, None, "retained.txt")
    trashed, = [row for row in listing(admin) if row["name"] == "retained.txt"]
    trash_download = download(admin, "file before moving to Trash", f"/entries/{trashed['id']}/content", "retained.txt")
    admin.request("move to Trash", "DELETE", f"/entries/{trashed['id']}", 204,
                  headers={"If-Match": trashed["etag"]})

    body = {"entry_id": notes["id"], "expiry_days": 1}
    active = admin.data("create active download link", "POST", "/share-links", 201, body)
    revoked = admin.data("create link to revoke", "POST", "/share-links", 201, body)
    check(link_path(active) != link_path(revoked), "Expected two distinct download links.")
    download(anonymous, "link before revocation", link_path(revoked), "notes.txt")
    admin.request("revoke download link", "DELETE", f"/share-links/{revoked['id']}", 204)
    return {"credentials": credentials, "active": active, "revoked": revoked, "trash_download": trash_download}


def inventory(entry):
    return {field: entry.get(field) for field in ("id", "parent_id", "name", "kind", "size", "modified_at")}


def snapshot(trust, state):
    admin, user, identities = sign_in_both(trust, state["credentials"])
    anonymous = Client(trust)

    accounts = admin.data("administrator account inventory", "GET", "/accounts", 200)
    check({(entry["account_id"], entry["handle"], entry["role"], entry["enabled"]) for entry in accounts}
          == {(entry["account_id"], entry["handle"], entry["role"], True) for entry in identities.values()}
          and len(accounts) == len(ACCOUNTS), "Enabled account inventory differed.")
    user.request("member denied account administration", "GET", "/accounts", 403)

    owner_rows = listing(admin)
    member_rows = listing(user)
    check([row["name"] for row in owner_rows] == ["Photos", "notes.txt"]
          and [row["name"] for row in member_rows] == ["member.bin"], "Private root inventory differed.")
    folder, notes = owner_rows
    nested = listing(admin, folder["id"])
    check([row["name"] for row in nested] == ["photos.bin"], "Nested folder inventory differed.")
    trash = items(admin, "administrator Trash listing", "/trash")
    check([row["name"] for row in trash] == ["retained.txt"]
          and trash[0]["size"] == state["trash_download"]["size"], "Retained Trash inventory differed.")
    check(items(user, "member Trash listing", "/trash") == [], "Member saw another account's Trash.")

    downloads = {}
    for client, other, row in [(admin, user, notes), (admin, user, nested[0]), (user, admin, member_rows[0])]:
        path = f"/entries/{row['id']}/content"
        downloads[row["name"]] = download(client, "owner file download", path, row["name"])
        check(row["size"] == downloads[row["name"]]["size"], "File listing size differed.")
        other.request("cross-account private download denied", "GET", path, 404)
        anonymous.request("anonymous private download denied", "GET", path, 401)
    user.request("cross-account folder listing denied", "GET", f"/folders/{folder['id']}/children", 404)
    admin.request("retained Trash excluded from live downloads", "GET", f"/entries/{trash[0]['id']}/content", 404)

    active, revoked = state["active"], state["revoked"]
    links = admin.data("owner download-link inventory", "GET", "/share-links", 200)
    check(len(links) == 1 and all(active[key] == value for key, value in links[0].items())
          and links[0]["id"] == active["id"], "Owner download-link inventory differed.")
    check(user.data("member download-link inventory", "GET", "/share-links", 200) == [],
          "Member saw another account's download links.")
    downloads["anonymous active link"] = download(anonymous, "anonymous active download link",
                                                   link_path(active), "notes.txt")
    anonymous.request("anonymous revoked download link", "GET", link_path(revoked), 404)
    return {
        "identities": identities,
        "roots": [root(admin)["id"], root(user)["id"]],
        "entries": [inventory(row) for row in [*owner_rows, *nested, *member_rows]],
        "trash": [dict(inventory(row), original_path=row["original_path"]) for row in trash],
        "downloads": downloads,
    }


def trash_bytes(trust, state):
    admin = Client(trust)
    admin.sign_in(state["credentials"]["fixture-admin"])
    original = state["snapshot"]["trash"][0]
    retained, = items(admin, "administrator Trash before restore", "/trash")
    check(dict(inventory(retained), original_path=retained["original_path"]) == original,
          "Unexpected retained file before restore.")
    path = f"/entries/{retained['id']}"
    admin.request("restore retained file", "POST", f"/trash/{retained['id']}/restore", 204,
                  headers={"If-Match": retained["etag"]})
    observed = download(admin, "restored Trash file", f"{path}/content", "retained.txt")
    check(observed == state["trash_download"], "Restored Trash file differed from the pre-upgrade download.")
    restored, = [row for row in listing(admin) if row["id"] == retained["id"]]
    admin.request("return restored file to Trash", "DELETE", path, 204, headers={"If-Match": restored["etag"]})
    returned, = items(admin, "administrator Trash after re-trashing", "/trash")
    check(inventory(returned) == inventory(retained) and returned["original_path"] == original["original_path"],
          "Retained file did not return to its original Trash inventory.")
    evidence(f"Retained Trash file restored, matched {observed['size']} bytes and SHA-256 {observed['sha256']}, and returned to Trash.")


def verify_revoked_grant(admin, member, library, entry_id):
    library_id = library["id"]
    path = f"/shared/entries/{entry_id}/content"
    manage = f"/entries/{library_id}/direct-grants"
    check(library_id not in [item["entry"]["id"] for item in member.data("member incoming shares", "GET", "/shared", 200)["items"]],
          "Revoked library remains discoverable.")
    member.request("member shared listing denied", "GET", f"/shared/folders/{library_id}/children", 404)
    member.request("member shared download denied", "GET", path, 404)
    check(admin.data("owner grant inventory without a grant", "GET", manage, 200)["items"] == [],
          "Unexpected grant in owner inventory.")


def verify_grants(trust, state, baseline):
    admin, member, _ = sign_in_both(trust, state["credentials"])
    library, entry_id = baseline["library"], baseline["entry_id"]
    libraries = items(admin, "owner library inventory", "/libraries")
    check([inventory(row) for row in libraries if row["id"] == library["id"]] == [library],
          "The named library changed.")
    verify_revoked_grant(admin, member, library, entry_id)
    check(download(admin, "owner named library download", f"/entries/{entry_id}/content", "notes.txt")
          == baseline["download"], "Owner named library bytes changed.")
    active = baseline["active"]
    incoming = member.data("member incoming shares", "GET", "/shared", 200)["items"]
    check([item["entry"]["id"] for item in incoming] == [active["library_id"]], "Active grant missing or unexpected shares.")
    check([item["id"] for item in items(member, "active shared listing", f"/shared/folders/{active['library_id']}/children")] == [active["entry_id"]], "Active grant listing differed.")
    check(download(member, "active shared download", f"/shared/entries/{active['entry_id']}/content", "notes.txt") == active["download"], "Active grant bytes changed.")
    evidence("Active and revoked library grants and bytes matched.")


def grants(trust, state):
    admin, member, _ = sign_in_both(trust, state["credentials"])
    anonymous = Client(trust)
    library = admin.data("create named library", "POST", "/libraries", 201, {"name": "Shared rehearsal"})
    library_id = library["id"]
    uploaded = upload(admin, library, "notes.txt")
    path = f"/shared/entries/{uploaded['id']}/content"
    children = f"/shared/folders/{library_id}/children"
    manage = f"/entries/{library_id}/direct-grants"

    verify_revoked_grant(admin, member, library, uploaded["id"])
    grant = admin.data("grant named library", "POST", manage, 200, {"handle": "fixture-member"})
    check(admin.data("owner grant inventory", "GET", manage, 200)["items"] == [grant],
          "Owner grant inventory differed.")
    incoming = member.data("member incoming shares", "GET", "/shared", 200)["items"]
    check([item["entry"]["id"] for item in incoming] == [library_id], "Granted library was not discovered.")
    check([item["id"] for item in items(member, "member shared library listing", children)] == [uploaded["id"]],
          "Granted library listing differed.")
    download(member, "member granted library download", path, "notes.txt")
    member.request("member private route still denied", "GET", f"/entries/{uploaded['id']}/content", 404)
    anonymous.request("anonymous named grant download denied", "GET", path, 401)
    admin.request("revoke named library grant", "DELETE", f"/direct-grants/{grant['id']}", 204)
    verify_revoked_grant(admin, member, library, uploaded["id"])
    observed = download(admin, "owner download after named grant revocation",
                        f"/entries/{uploaded['id']}/content", "notes.txt")
    evidence("Named library baseline passed. The member discovered, listed and downloaded the grant, then lost access on revoke. Owner bytes still matched.")
    active_library = admin.data("create active library", "POST", "/libraries", 201, {"name": "Active rehearsal"})
    active_entry = upload(admin, active_library, "notes.txt")
    admin.data("grant active library", "POST", f"/entries/{active_library['id']}/direct-grants", 200, {"handle": "fixture-member"})
    return {"library": inventory(library), "entry_id": uploaded["id"], "download": observed,
            "active": {"library_id": active_library["id"], "entry_id": active_entry["id"], "download": observed}}


def save(path, value):
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(value, output)


def main():
    phase, directory = sys.argv[1:]
    fixture = Path(directory)
    trust = fixture / "trust.pem"
    baseline = fixture / "expected.json"
    grant_baseline = fixture / "expected-grants.json"
    if phase == "ready":
        client = Client(trust)
        deadline = time.monotonic() + 30
        while True:
            try:
                client.request("readiness", "GET", "/readyz", 204)
                return
            except CheckFailed:
                if time.monotonic() >= deadline:
                    raise CheckFailed("Certificate-verified readiness timed out.") from None
                time.sleep(0.25)
    elif phase == "seed":
        state = seed(trust, sys.stdin.read().strip())
        state["snapshot"] = snapshot(trust, state)
        save(baseline, state)
        evidence("Seeded two local accounts, a nested folder, four original files, retained Trash and active/revoked download links.")
    elif phase in ("verify", "trash-bytes", "grants", "verify-grants"):
        state = json.loads(baseline.read_text())
        if phase == "grants":
            save(grant_baseline, grants(trust, state))
            return
        if phase == "verify-grants":
            verify_grants(trust, state, json.loads(grant_baseline.read_text()))
            return
        if phase == "trash-bytes":
            trash_bytes(trust, state)
            return
        check(snapshot(trust, state) == state["snapshot"], "Read-equivalent inventory or download comparison differed.")
        evidence("Baseline matched for both fresh sign-ins, private inventory, retained Trash, account isolation, exact file downloads and anonymous active/revoked links.")
        for label, download_result in enumerate(state["snapshot"]["downloads"].values(), start=1):
            evidence(f"Download {label} matched {download_result['size']} bytes and SHA-256 {download_result['sha256']}.")
    else:
        raise CheckFailed("Unknown fixture phase.")


if __name__ == "__main__":
    try:
        main()
    except CheckFailed as error:
        sys.exit(str(error))
    except Exception as error:
        # Exception details can contain credentials or response data.
        sys.exit(f"Fixture driver failed ({type(error).__name__}); private details withheld.")
