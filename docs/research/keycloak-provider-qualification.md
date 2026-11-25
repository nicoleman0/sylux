# Keycloak provider qualification

SYL-114 tests the proposed native-user profile against the account lifecycle
policy. The evidence covers one disposable Keycloak instance on one host.
[SYL-114](https://linear.app/corvus-dev/issue/SYL-114),
[OIDC proposal](sso-first-integration.md),
[lifecycle proposal](external-account-lifecycle.md).

## Reproduce

With the repository's Node and npm versions, OpenSSL 3 or newer, and a local
Docker daemon that publishes bridge ports on loopback, run from the repo root.

```sh
npm --prefix web ci
node web/node_modules/playwright/cli.js install chromium --only-shell
scripts/qualify-keycloak.sh
```

The runner pins the 26.6.4 image by OCI index digest and starts Keycloak in
development mode on a dedicated Docker bridge with IP masquerading disabled.
The container publishes HTTPS only on loopback. The callback binds a separate
loopback port. The driver uses the web lockfile and Playwright's selected
Chromium build. The run includes a five-minute sampling window.
[Runner](../../scripts/qualify-keycloak.sh),
[driver](../../scripts/qualify-keycloak.mjs),
[Keycloak release](https://www.keycloak.org/2026/06/keycloak-2664-released),
[Docker bridge documentation](https://docs.docker.com/engine/network/drivers/bridge/).

The HTTPS client verifies the loopback address and chain with the disposable CA.
Chromium uses a process-local exception for the ephemeral leaf's public key and
allows page requests only to the two fixture origins. The launcher verifies the
leaf's chain and loopback SAN before startup. The fixture makes no trust-store
changes. Secrets and certificates are generated per run. The runner discards
provider logs and removes the container, network and temporary certificates
on exit.

## Profile and checks

- One native-user realm with the default browser flow and a synthetic password.
  Provisioning and mutations use a separate temporary administrative service
  account. [Keycloak Admin REST API](https://www.keycloak.org/docs-api/26.6.4/rest-api/).
- One confidential RP with a single literal HTTPS callback,
  `client_secret_basic`, required PKCE S256, code flow and `openid` scope.
  The fixture checks the effective settings through the administrative API.
  The RP has no refresh, offline, password-grant, implicit or service-account
  path. Keep Keycloak's default `basic` client scope on the RP for its
  `auth_time` mapper. The browser request still sends only `openid`.
  [Administration guide](https://www.keycloak.org/docs/26.6.4/server_admin/),
  [26.6.4 scope source](https://github.com/keycloak/keycloak/blob/26.6.4/services/src/main/java/org/keycloak/protocol/oidc/OIDCLoginProtocolFactory.java#L467-L478).
- Live discovery must match the exact issuer, expected endpoints and protocol
  capabilities. The driver checks callback state and issuer, uses each
  transaction once, and validates the ID token's RS256 signature against the
  advertised JWKS, exact issuer, audience, authorized party, subject, nonce
  and times. Fresh attempts require `max_age=0` and signed `auth_time`
  within the attempt. A separate silent SSO control measures reuse of earlier
  authentication. See [OIDC Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html#ProviderConfigurationValidation)
  and [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html#AuthRequest).

## Single-host results

Run on 25 November 2026 with macOS 26.6.2 arm64, Docker Desktop daemon
29.8.2 Linux arm64, Node 24.22.2, Playwright 1.65.0 and Chromium Headless Shell
158.0.8127.18. The Keycloak image index was
`sha256:0aae0de7fca85525f727d3354df17896092de8bb26ae4c12d89c77e5df8cbce4`.

| Journey | Observation |
| --- | --- |
| Identity continuity | After admin readback confirmed the rename, a fresh browser signed in with the renamed username and received the same verified issuer and subject. Deleting and recreating the same username produced a different verified subject under the same issuer. |
| Freshness with existing SSO | Silent SSO returned the earlier signed authentication time. After crossing a whole-second boundary, `max_age=0` displayed the provider password form and the completed attempt returned a newer signed `auth_time`. |
| Effective and observed code limit | Keycloak reported a 20 s code lifetime. A code issued before disablement got `invalid_grant` with `User disabled` at an age of at most 1.35 s. A form opened before disablement also denied after submission. The enabled-user expiry control rejected an unredeemed code at an age of at least 22 s. |
| Previously issued ID token | Keycloak issued a token with a 60 s signed lifetime. Its signature still verified offline after disablement with 57 s to expiry. The run passed its signed expiry before sampling ended. The token was held only to observe offline verification and expiry. |
| Outage and recovery | With the container stopped, new browser authorization and an in-flight token exchange both failed at the transport boundary in 0.13 s, with a 5 s limit per request. Restart, validated discovery and a new fresh proof with the same issuer and subject took 4.55 s from restart initiation. |

The admin disable write took 0.01 s to acknowledge. The first pair of
fresh browser attempts completed denial by 0.50 s after acknowledgement.
Each pair used a clean browser context and a separate context initialized
from the pre-disable SSO cookie state. Both attempts received a
provider-rendered disabled-account denial at every sample, scheduled immediately
and at 5, 30, 60, 120, 180, 240, 290 and 300 s. The last pair finished at
300.29 s after acknowledgement.

These observations are consistent with the proposed **P ≤ 5 minutes** for
the exercised paths on this host. Sampling gives an upper bound on when
denial was first observed in this run. A worst-case P across all paths and
operating conditions, access-token use after disablement, sub-second
freshness, and the proposed **P + 15 minutes ≤ 20 minutes** Sylux access
bound remain unverified.

## Operator decisions before SYL-115

- Confirm the production provider version and topology, the canonical issuer
  and callback, network and CA ownership, who rotates secrets, and who can
  verify the intended person for explicit provisioning and linking.
- Name the required MFA and assurance policy, including reauthentication,
  enrolment and recovery. Qualify that policy and any required assurance
  claims in the chosen browser flow. This fixture uses a password.
- Choose an acceptable authentication attempt, fixed external-session expiry
  and outage behavior. The fixture's request and token settings are experiment
  inputs. SYL-88 proposes a 15-minute fixed external session and separate
  local administrator access and recovery during provider failure.
- Decide whether a qualified upstream bound is required or explicit local
  disablement is the authoritative removal procedure. If a finite upstream
  bound is required, qualify the intended topology and every eligible proof
  path, including federation/cache/replication and authentication in flight.
  Resolve any required event or polling mechanism and the treatment of open
  requests before claiming an end-to-end bound.

SYL-98 owns AD bridge evidence. Sylux linking, session admission and expiry,
local recovery, logout, audit, role decisions, revocation during downloads and
access to stored files require product verification under their own issues.
The test does not assess upgrades, clustered operation or alternative browsers.
