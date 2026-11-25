# Keycloak sign-in

Sylux supports one optional Keycloak registration. The qualified setup is
single-node Keycloak 26.6.4 with native realm users, a confidential client
and a browser flow that requires a password and OTP. A local Sylux administrator
approves each account. Keep a separate local administrator and confirm that
its password works before enabling Keycloak.

## Configure Keycloak

1. Serve the realm at its canonical HTTPS issuer, for example
   `https://id.example.org/realms/files`. Use a URL reachable by Sylux and
   its users' browsers. Sylux uses the realm's own discovery, authorization,
   token and signing-key endpoints at that same origin.
2. Create an OpenID Connect client with **Client authentication** and
   **Standard flow** enabled. Register the literal redirect URI
   `https://files.example.org/auth/oidc/callback`. Require PKCE `S256`,
   `client_secret_basic` and RS256 ID tokens. Retain the default `basic`
   client scope so the signed ID token includes `auth_time`. Disable direct
   access grants, implicit flow, service accounts and refresh/offline tokens.
   Use the default public subject for the native realm profile.
3. Bind a dedicated browser flow to the realm or client. Set **Username
   Password Form** and **OTP Form** to **Required**. Leave out Cookie,
   identity-provider redirects and alternative flows that could accept an
   earlier session or skip the required factor. Establish authenticator
   enrolment and recovery in Keycloak. Test with disposable enrolled and
   unenrolled users. An enrolled user must see the OTP prompt after the
   password, even with a prior Keycloak session. Password alone and an
   invalid OTP must leave Sylux signed out. Complete any required enrolment
   through the operator's trusted process.
4. Verify the client and authentication flow again after edits. Keycloak
   enforces MFA. Sylux requires fresh signed authentication time and
   ignores provider roles, groups, email and assurance claims.

See the [Keycloak 26.6.4 administration guide](https://www.keycloak.org/docs/26.6.4/server_admin/)
and [disposable profile evidence](research/keycloak-provider-qualification.md).

## Configure Sylux

Write a JSON file with this shape. Use absolute paths to regular files readable
by the Sylux service. Use the hostname and port from the browser's Sylux URL for
the public origin. Use the canonical URL spelling with no explicit default
HTTPS port, query, fragment or trailing slash on the issuer.

```json
{
  "issuer": "https://id.example.org/realms/files",
  "client_id": "sylux",
  "callback": "https://files.example.org/auth/oidc/callback",
  "client_secret_file": "/etc/sylux/oidc/client-secret",
  "ca_file": "/etc/sylux/oidc/keycloak-ca.pem"
}
```

The callback must exactly equal the public origin plus `/auth/oidc/callback`
and match the single URI registered in Keycloak. Put the client secret alone
on one line in a file owned by the Sylux service identity with mode `0600`
or `0400`. Keep the directory private and leave the secret out of JSON,
command arguments, environment variables and logs. Sylux uses only the CA
certificates in the configured PEM for its outbound Keycloak HTTPS requests.
The browser also needs to trust both HTTPS origins.

Append these arguments to the current `sylux serve` command and restart.

```text
--public-origin https://files.example.org --oidc-config /etc/sylux/oidc/provider.json
```

For the [Docker installation](docker-beta.md), this `compose.override.yaml`
extends the default service. Use `/run/sylux/oidc/` paths in JSON and UID
10001 for the secret file. Keep the existing volume and TLS mounts.

```yaml
services:
  sylux:
    volumes:
      - /etc/sylux/oidc:/run/sylux/oidc:ro
    command:
      - serve
      - --root
      - /var/lib/sylux
      - --listen
      - 127.0.0.1:3000
      - --tls-cert
      - /run/sylux/tls/cert.pem
      - --tls-key
      - /run/sylux/tls/key.pem
      - --public-origin
      - https://files.example.org
      - --oidc-config
      - /run/sylux/oidc/provider.json
```

A host or proxy must serve Sylux's password sign-in without a Keycloak gate.
Configure access logs to omit the query string on the callback route.

Without the flag, new Keycloak sign-ins are off. A missing or invalid
configuration file, unreadable secret or CA, or changed registration logs a
short diagnostic and leaves local sign-in available. A provider that cannot
be contacted denies new sign-ins. None of these cases removes the registration,
revokes sharing or changes the existing external session deadline.

## Approve sign-in and convert an account

1. In the configured Keycloak realm, locate the intended user by a trusted
   administration channel. Verify the person through a separate trusted
   contact process. Copy the user's immutable ID from the user record in
   Keycloak administration. For this profile, that ID is the exact OIDC
   subject. Verify their MFA enrolment and that the configured flow actually
   requires it. Do not copy a display name, email address or user-supplied ID.
2. Open **Settings**, then **Accounts**. Choose **Approve sign-in** under
   **Keycloak sign-in** and enter the exact subject. Select **New account**
   and provide an unused Sylux name, or select the intended existing account.
   A pending approval expires after 24 hours. Use **Cancel approval** to
   withdraw it early. An approved new account exists only after the person
   completes Keycloak authentication.
3. For a new account, the person enters the approved Sylux name on the
   sign-in page and selects **Sign in with Keycloak**. To convert an
   existing local account, they first sign into that account with their
   local password. In **Account**, select **Switch to Keycloak**, enter the
   current local password, and finish in that browser. Conversion keeps the
   account, files, Trash and sharing, removes its password and revokes its
   earlier sessions and WebDAV app passwords. Administrators keep local
   credentials.
4. To revoke a linked account, use **Revoke link** in Settings. To block
   access immediately while keeping its link, use **Disable** on the
   account. Both end its direct shares and withdraw its published
   libraries. A local administrator can approve the original issuer and
   subject for the same disabled account after revocation. The person must
   complete fresh proof, and owners must explicitly recreate sharing.

## Rotate or remove the registration

- **Rotate the client secret or update CA trust.** Keep the issuer, client ID
  and callback unchanged. Replace the protected secret file with the value
  generated by Keycloak and restart Sylux. Validate a fresh sign-in. Existing
  links, pending approvals, active sessions and sharing survive. In-flight
  attempts must restart after a Sylux restart.
- **Remove or replace the registration.** Stop Sylux and make a verified
  backup. As the service identity, run
  `sylux admin remove-oidc-provider --root /path/to/state`. In Docker, use
  the stopped service's image and data mount as in the [recovery command](docker-beta.md#recover-administrator-access).
  This atomically revokes links, pending approvals and sessions. Accounts
  left without a credential are disabled, direct grants to and from them
  are revoked, and their published libraries are withdrawn. Files and owner
  IDs remain. Start with the flag omitted for removal, or point it to the
  replacement configuration. Handle a Keycloak realm rebuilt at the same
  URL as a replacement. There is no automatic reassignment to a different
  subject or issuer. Retain the original identity data and backup if owners
  need their original key restored. Confirm the account and sharing state
  before retrying if the command reports an uncertain commit.

## Session limits and recovery

Each attempt lasts at most five minutes. Sylux requests `max_age=0` and
checks the signed `auth_time` against its attempt with at most five seconds
of clock skew. Synchronize the host clocks. The tolerance applies to
authentication evidence only. The session deadline is fixed at 15 minutes
from the start of the Sylux attempt. Activity, clock tolerance, retries and
provider tokens cannot extend the stored deadline. Sign in again for a new
session.

During a Keycloak outage, new sign-in and renewal fail. Existing sessions
can make authorized requests up to their fixed deadline, unless locally
revoked sooner. Sharing stays intact. Local disablement applies at the next
authorization decision after the change commits. A download already admitted
can continue and bytes already sent cannot be recalled. Uploads and other
mutations still follow Sylux's existing admission and commit rechecks.
Upstream timing is measured in the disposable fixture. Use local disablement
when access must stop at the next authorization decision.

**Sign out** revokes the Sylux session locally even during a provider outage.
It leaves Keycloak and other applications' sessions alone. If the separate
local administrator loses access, use [host administrator recovery](docker-beta.md#recover-administrator-access).
It needs the stopped service, a verified backup and control of the service
identity. Keycloak is not involved.

Sylux records successful link lifecycle changes and session issuance in its
protected metadata using internal references. Failed proofs use bounded
requests and generic responses. Sylux does not store ID, access or refresh
tokens or provider passwords. Protect metadata and backups, which contain the
issuer and subject mapping and the audit records for the life of the database.
