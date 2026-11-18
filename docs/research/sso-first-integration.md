# First external sign-in integration

SYL-86 research proposal. Evidence checked **18 November 2026**.
No external sign-in support has been implemented or tested in Sylux by this work.

## Recommendation

Start with **one administrator-configured OIDC provider**, using a confidential
server client, authorization code flow and PKCE S256. Use **Keycloak as the first
disposable interoperability target**. This selects a test target, not a required
operator vendor. Keep local sign-in and an existing local administrator available.

This is the smallest useful proposal for the self-hosters and small groups in
[PRODUCT.md](../../PRODUCT.md): users can reuse an IdP without Sylux handling their
directory passwords or deploying a directory connector. It still adds provider
configuration, outbound HTTPS and another service on the sign-in path.

Keycloak documents confidential clients, code flow and enforced S256 in its
[client settings](https://www.keycloak.org/docs/latest/server_admin/index.html#_oidc_clients).
Its [OIDC endpoints](https://www.keycloak.org/securing-apps/oidc-layers) include
discovery, authorization, token, signing keys and logout. Those are concrete
protocol capabilities to exercise, not proof of Sylux compatibility.

## Scope and ownership

- [SYL-86](https://linear.app/corvus-dev/issue/SYL-86) owns protocol choice,
  configuration and login/callback/logout research.
- [SYL-87](https://linear.app/corvus-dev/issue/SYL-87) owns direct LDAP/AD
  connectivity, secure binds, searches, directory identity and failure behavior.
  See the [LDAP/AD recommendation](ldap-ad-authentication.md).
  An AD-connected IdP is an alternative route, not direct LDAP support in Sylux.
- [SYL-88](https://linear.app/corvus-dev/issue/SYL-88) owns stable account links,
  provisioning, collision rules, roles/groups, deprovisioning, session revocation
  and file ownership. The interface needs below are inputs for reconciliation,
  not settled lifecycle policy. Neither related issue is implemented here.

## Protocol comparison

Relative effort is a design assessment for Sylux, not a measured benchmark.

| Candidate | Operator setup and dependencies | Trust boundary and failure modes | Fit and decision |
| --- | --- | --- | --- |
| OIDC code + PKCE | Register a web client, exact callback, issuer and secret. Sylux needs an OIDC/JWT validation library, bounded HTTPS client, discovery/key cache and short-lived login transactions. | IdP proves identity; Sylux decides account access. Risks include callback substitution, replay, issuer confusion, bad signature/key handling and outbound URL abuse. Provider downtime blocks fresh external sign-in. | Best first fit. Existing self-hosted IdPs can remain separate from Sylux. AD can sit behind an IdP. One protocol avoids vendor-specific authentication code. |
| SAML 2.0 browser SSO | Exchange SP/IdP metadata, entity IDs, assertion consumer service URL, bindings and signing certificates. Needs a maintained XML signature/schema stack, assertion replay cache and certificate rotation handling. | Validate the signed element actually consumed, issuer, audience, recipient, request correlation and validity. XML signature wrapping adds a distinct parser boundary. | Useful for an operator with a SAML-only IdP. Defer until such a need is confirmed; adds a second validation stack without an identified first user. |
| Trusted proxy headers | Operator runs an authentication proxy/outpost, configures protected routes and headers, and prevents direct access to Sylux. Sylux still needs stable identity mapping and sessions/authorization. | The proxy and every path to the upstream become part of the identity boundary. Forged headers or a bypass route can impersonate users. Proxy outage can block local administrator browser sign-in too; host CLI recovery is unaffected. | Potentially short application code, greater topology dependence. Defer native header trust. Putting a proxy gate in front of today's local login alone does not implement external account sign-in. |

OIDC's [core validation rules](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation)
define token checks; the [OAuth security BCP, sections 2.1 and 4](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.1)
supports PKCE for web clients and strict redirect protection. The proposed profile
excludes implicit and password grants.

For SAML, [OASIS profiles sections 4.1.4.3 and 4.1.4.5](https://docs.oasis-open.org/security/saml/v2.0/saml-profiles-2.0-os.pdf)
require response processing and replay protection. [OWASP's SAML guidance](https://cheatsheetseries.owasp.org/cheatsheets/SAML_Security_Cheat_Sheet.html#validate-signatures)
describes schema/signature validation and signature-wrapping defenses. Authentik's
[SAML provider documentation](https://docs.goauthentik.io/add-secure-apps/providers/saml/)
confirms metadata exchange and Redirect/POST bindings, so SAML is a real candidate.
If reconsidered, prefer SP-initiated login; unsolicited IdP login and single
logout require their own bounded design and tests.

Authentik's [proxy modes and headers](https://docs.goauthentik.io/add-secure-apps/providers/proxy/)
document forward auth and an upstream user ID. Its [nginx example](https://docs.goauthentik.io/add-secure-apps/providers/proxy/server_nginx/)
copies identity from the auth subrequest into upstream headers. A future Sylux
integration would need explicit trusted peer authentication or isolated transport,
replacement of all incoming identity headers, rejection of missing/duplicate
identity, and bypass tests on every listener. A trusted forwarded IP alone is
insufficient identity evidence. Local administrator browser sign-in would need a
separately protected route, not a public bypass with spoofable headers. The
[host CLI recovery path](../docker-beta.md#recover-administrator-access) does not
use the proxy and remains available during a proxy outage.

## Researched compatibility

All rows are **documentation evidence only**, checked on the date above.
No provider was launched, no tenant was accessed, no discovery endpoint from a
running deployment was queried, and no Sylux sign-in was performed.

| Provider or route | Content inspected and supported inference | Qualification still needed |
| --- | --- | --- |
| Keycloak, first target | Client settings cover confidential code/S256; endpoint documentation gives realm discovery and JWKS/logout paths. | Pin an image version and digest, register a client, inspect actual metadata and complete every first-target journey below. No version is certified here. |
| authentik, second candidate | [OAuth2 provider](https://docs.goauthentik.io/add-secure-apps/providers/oauth2/) explicitly documents OIDC, code flow, PKCE, discovery, JWKS and end-session endpoints. | Check issuer mode, subject stability, signing algorithm, client authentication and logout behavior against a pinned version. Do not publish a supported-provider claim yet. |
| AD through Keycloak | [LDAP/AD federation](https://www.keycloak.org/docs/latest/server_admin/index.html#_ldap) documents the directory bridge. Sylux would speak OIDC to Keycloak, not LDAP to AD. | SYL-87 must assess the directory side. Disabled AD users, federation caching and password-expiry behavior need a real disposable AD fixture. OIDC success with local Keycloak users proves none of these. |
| AD FS | Microsoft's [OIDC/OAuth concepts](https://learn.microsoft.com/en-us/windows-server/identity/ad-fs/development/ad-fs-openid-connect-oauth-concepts) applies to AD FS 2016 and later and describes confidential web clients. | Exact server version, PKCE/client authentication, claims and logout need verification. Do not weaken the first profile to accommodate an untested AD FS deployment. |
| Microsoft Entra ID with hybrid AD | Microsoft's [code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow) covers OIDC/code/PKCE; [Entra Connect](https://learn.microsoft.com/en-us/entra/identity/hybrid/connect/whatis-azure-ad-connect) describes hybrid authentication/synchronization options. | Requires an operator tenant/app registration and consent review. Use a specific tenant issuer, not an unbounded multi-tenant endpoint. AD-to-cloud setup, cost and policies are outside this recommendation. |

OIDC reduces Sylux's directory-specific surface. It does not remove the operator's
IdP/database, certificate, backup and upgrade work. Keycloak's
[container guide](https://www.keycloak.org/server/containers) distinguishes
development startup from production configuration. Do not bundle a production
IdP into Sylux to make the first test pass.

## Proposed configuration contract

These are requirements for a future implementation, not existing flags or files.
Configure one issuer through an administrator-controlled server configuration.
Do not accept an issuer URL from a login form.

| Input | Required behavior |
| --- | --- |
| Enablement and display label | Disabled by default; an explicit external sign-in button alongside local login. No forced redirect loop. |
| Issuer URL | Exact HTTPS issuer, including realm/path. Derive discovery from it; require the returned issuer to match. No wildcard or email-domain discovery. |
| Client ID and client authentication | Registered confidential client. First profile uses `client_secret_basic` for simpler operator setup without client signing-key registration; qualify provider metadata before enabling it. This shared-secret tradeoff departs from [RFC 9700 section 2.5](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.5), which recommends asymmetric client authentication (mTLS or `private_key_jwt`) to reduce shared-key leakage risk. Defer `private_key_jwt` qualification as later hardening. |
| Client secret reference | Read a restricted server-side secret file; never send it to the web bundle, command arguments or logs. Define rotation and configuration reload behavior before implementation. |
| Public Sylux origin | Canonical HTTPS origin used to generate one exact callback and one post-logout URI. Never build these from arbitrary Host/Forwarded headers. |
| Scopes and claims | Require `openid`; request profile/email only if the agreed lifecycle/display contract needs them. No `offline_access`, directory scopes or group scope by default. |
| TLS trust and destinations | Validate hostname and certificate chains. Allow a configured private CA for homelab IdPs, never a skip-verification switch. Approve required private-network destinations explicitly. |
| Session/admission policy | Must reference SYL-88's approved mapping, provisioning and revocation contract. No default external administrator mapping. |

The [discovery specification, sections 4.3 and 7](https://openid.net/specs/openid-connect-discovery-1_0.html#ProviderConfigurationValidation)
requires issuer consistency and TLS checks. For Sylux, additionally bound fetch
time/size, redirects and key refreshes. Discovery metadata must not become an
arbitrary network fetch mechanism: validate fetched endpoint destinations against
operator-approved origins, reject credentials/fragments, and prevent redirect
escapes. Some IdPs use different endpoint hosts; make that an explicit operator
allowlist rather than assuming every endpoint shares the issuer host.

Proposed first limits, to validate during implementation: 5-minute single-use
login transactions, at most 1,024 outstanding transactions, 10-second total
outbound request deadline, 1 MiB metadata/JWKS/token response limit, 60-second
clock tolerance and one bounded key refresh on an unknown key ID. These are design
starting points, not measured requirements or standardized defaults. Set an
explicit finite metadata/key cache lifetime; reject new logins when required keys
are unavailable or beyond that lifetime. Final cache TTL is an unresolved input.

## User journeys and validation boundary

The numbered steps are a proposed implementation contract.

1. **Start login.** A user chooses the configured provider. Protect initiation
   against login CSRF. Create random state, nonce and PKCE verifier in a bounded
   server transaction tied to the initiating browser with a secure, HttpOnly,
   SameSite=Lax cookie. Use a top-level GET query callback for the first profile.
   Store only an allowed local return destination. Never put credentials or file
   paths into state or authorization URLs.
2. **Callback.** Accept only the configured callback and expected parameters;
   reject duplicates, missing/expired/mismatched browser state, provider errors
   and replay. Atomically consume the transaction before code exchange, including
   failure paths. Exchange the code server-side with PKCE and the same redirect
   URI. A network failure requires a fresh login, not reuse of the consumed code.
3. **Validate identity.** Use the library's complete OIDC validation. Require
   issuer, audience/authorized-party checks, expiry, nonce, acceptable issue time
   and issuer signing keys. First profile pins RS256, rejects `none` and unexpected
   algorithms, and always verifies signatures. Ignore token-supplied key URLs.
   If UserInfo becomes necessary, match its subject to the validated ID token.
   [Core sections 3.1.3.7, 5.3 and 5.7](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation)
   are the basis; stricter profile restrictions here are Sylux proposals.
4. **Admit an account.** Hand the verified `(issuer, subject)` to SYL-88's local
   account policy. Email and username are display data, never automatic link
   keys. Reject unknown/colliding identities until provisioning is agreed. Recheck
   account enablement and authority when issuing a fresh local session, with no
   IdP network calls inside SQL transactions. Rotate the anonymous session. The
   browser receives a Sylux session, never a provider bearer token for file APIs.
5. **Sign out.** Revoke the local session and clear its cookie first, using the
   existing CSRF protection. Local sign-out must succeed with the IdP down. A
   separate explicit provider sign-out can use discovered `end_session_endpoint`,
   a registered post-logout URI and fresh correlated logout state. Store any
   `id_token_hint` server-side only for the local session lifetime, but recognize
   that sending it to the provider transits the browser in a Sylux response.
   Prefer a form POST with a non-cacheable response; do not persist the hint in
   browser storage and redact it from Sylux, proxy and IdP logs. GET would also
   expose it in URLs, potentially browser history and URL/access logs; record
   that exposure if qualifying GET. This logout-only transit is the explicit
   exception to V1's response restriction, checked in V6. If unavailable/failing, say
   local sign-out completed and provider sign-out was not confirmed. The
   [RP-Initiated Logout standard](https://openid.net/specs/openid-connect-rpinitiated-1_0.html#RPLogout)
   defines that separate request. Local sign-out alone leaves IdP SSO active, so
   choosing external sign-in again may immediately sign the user back in.
6. **Provider outage.** Discovery, TLS, JWKS, token or provider-policy failures
   never create a session or fall back to password authentication for that
   external identity. Show a retryable sign-in failure and keep local login
   usable. Do not erase valid local sessions merely because discovery fails.
   Existing external sessions follow the finite expiry/revocation policy agreed
   with SYL-88; no claim of immediate upstream-disable propagation is justified
   by OIDC login alone.
7. **Local administrator recovery.** Before enabling SSO, verify a separate local
   administrator works without the IdP. Preserve its local login and host recovery
   path. [Existing recovery](../docker-beta.md#recover-administrator-access)
   stops Sylux, verifies a backup and resets an existing administrator password
   through stdin; it enables that account and revokes its sessions. Follow that
   guide rather than editing SQLite or reopening setup. Future configuration
   errors must disable external login without taking down local recovery.

The current [login/session code](../../src/http/auth.rs) rechecks account state
under the session insertion transaction; [account administration](../../src/http/accounts.rs)
revokes sessions on authority changes. These are source observations, not new
runtime verification. Any future callback must preserve those boundaries and the
existing [authority verification](../../.agents/skills/verify-sylux-authority/SKILL.md).

## Lifecycle handoff and release gates

Ask SYL-88 to resolve these before external sign-in ships:

- An external identity maps to one immutable local account ID. Decide explicit
  provisioning versus first-login creation, linking proof, collisions and issuer
  changes. No automatic merge with a local account that shares an email address.
- First slice should use administrator-approved links and local role assignments;
  this is a recommendation awaiting SYL-88, not an implemented provisioning rule.
  No IdP group may silently grant administrator authority.
- Set a finite session lifetime and acceptable maximum delay from upstream
  disablement to loss of access. Local disablement must prevent issuance and
  invalidate sessions; upstream disablement needs reauthentication, a validated
  logout/event channel or another agreed signal. Do not infer it from token expiry
  or assume token introspection revokes a separate Sylux session.
- Decide behavior for local passwords on external accounts, unlinking, provider
  deletion and re-provisioning. Preserve files and stable ownership on identity
  removal; never reassign them to a newly created same-name user.

## Disposable verification plan

**Planned, not run.** No implementation is available to exercise these journeys.

Use an isolated test host with disposable Sylux storage, a pinned Keycloak image
and digest, a new realm, two synthetic users and one local Sylux administrator.
Use trusted test TLS and DNS names reachable from both browser and Sylux. Record
versions, image digest, discovery metadata, redacted client export, exact Sylux
commit, commands and results. Destroy the realm, secrets and storage afterwards.
Never point the fixture at a real tenant or production directory.

Register one confidential client with Standard Flow on and S256 required; disable
implicit, direct password and service-account flows. Register only the exact
callback/post-logout URIs. Inspect live metadata for issuer, code flow, signing
algorithms, authentication methods and logout endpoint. Use actual Keycloak for
the positive paths and a controlled test issuer/fault proxy for malformed tokens
and network failures. A mock issuer alone is not provider compatibility evidence.

| Check | Action and required observation |
| --- | --- |
| V1 Login and reload | Sign in as approved synthetic user A through the real provider, land on Files, reload and upload/download a known fixture. No provider tokens in browser storage or logs, or in responses except the explicit logout-only ID-token hint transit checked in V6. |
| V2 Isolation | Sign in as B in a separate browser context. Neither can list, download, mutate or recover the other's files, including guessed IDs. Unknown identity and same-email identity cannot acquire A's account. |
| V3 Callback attacks | Change/remove state, nonce, browser cookie, code verifier, issuer, audience, signature, key ID, algorithm and timestamps independently. Try duplicate parameters, transaction expiry, callback replay and two simultaneous callbacks. Each rejects without session issuance; the valid control still succeeds. |
| V4 URL and transport | Reject unregistered callback/return URLs, Host/Forwarded manipulation, discovery issuer mismatch, endpoint redirect escapes, untrusted/expired certificates and unapproved destinations. Exercise configured private CA success as a separate control. |
| V5 Outage and rotation | Stop provider; delay/drop discovery, token and JWKS responses; return oversized/malformed bodies; rotate a signing key. Check bounded failures, capped refresh, cache expiry and successful fresh login after recovery. Local login/logout stays usable. |
| V6 Logout | Confirm old local cookie fails after logout with provider up and down. Separately test provider sign-out, rejected post-logout redirect, canceled provider logout and logout-state replay. Verify the V1 exception is limited to the ID-token hint in the logout form POST, with a non-cacheable response, no persistent browser storage and no token logging; record URL/history and access-log exposure if qualifying GET instead. Record whether the next sign-in reuses IdP SSO; never report global logout from a local-cookie check. |
| V7 Authority and recovery | Disable/demote A locally while callback exchange is held; issuance/access must respect current authority. With IdP unavailable, recover the existing local administrator using the guide; verify old credentials/session fail, setup stays closed and known file checksums survive. |
| V8 Upstream lifecycle | Disable A at the IdP and measure existing/new session behavior against SYL-88's agreed bound. Only add AD disablement tests with SYL-87's disposable directory target. Record the actual delay, not an assumed instant revocation. |

During future implementation, start with relevant Rust tests, then real TLS
browser journeys and the authority harness. Refresh `web/dist` before Cargo after
any UI changes, as AGENTS.md requires. Re-run required CI before merge. These
future checks are not evidence from this research task.

## Bounded implementation candidates and deferrals

Candidates for later approval, not new issues or authorization:

1. Qualify a maintained Rust OIDC library against code/S256, discovery, strict
   validation and bounded network controls. Check its license, maintenance,
   advisory status, toolchain fit and dependency cost before selection. Deliver a
   disposable Keycloak round trip and adversarial callback tests without account
   provisioning. No package/version is committed by this document.
2. After SYL-88 reconciliation, add the one-provider configuration, approved
   identity links and local-session issuance with V1–V5 and V7–V8 acceptance.
   Preserve local recovery and existing authority/storage isolation.
3. Add explicit provider sign-out with V6, keeping local logout independent.
   Qualify authentik as a second provider only against the same documented
   profile and tests; avoid provider-specific branches without evidence.

Defer SAML, trusted-header login, multiple providers, dynamic registration,
refresh/offline tokens, front/back-channel logout, SCIM, group synchronization
and automatic role mapping. SYL-88 assesses which lifecycle signals or MFA policy
are actually needed; urgent revocation requirements may make a logout signal
necessary before release. Direct LDAP remains SYL-87. No new enterprise identity
platform, directory writes or production IdP deployment is proposed.

Unresolved operator needs: which IdPs and exact versions are already deployed;
whether AD access must stay entirely on-premises; availability of a disposable
AD/Entra environment; private CA/network topology; provisioning preference;
acceptable revocation delay and outage behavior; whether provider-wide logout
is wanted; and who rotates client secrets and maintains the IdP. Recheck the
linked live documentation and actual metadata when selecting test versions.

## Evidence limits

This work read primary protocol/vendor/security documentation and local source,
reviewed links and checked the document diff. It did not run application builds,
unit tests, browser tests, provider containers, TLS sign-in or recovery commands.
No implemented support, conformance, deployment qualification or vendor
endorsement follows from this proposal. The evidence date records retrieval,
not a claim that the documents or all provider releases share that date.
