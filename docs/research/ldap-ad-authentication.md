# LDAP and Active Directory authentication

SYL-87 research proposal. Primary evidence checked **18 November 2026**.
This document proposes behavior; it adds no LDAP, AD or SSO support to Sylux.

## Recommendation and current boundary

Keep the [SYL-86 first integration](sso-first-integration.md): one configured OIDC
provider, with Keycloak as the first disposable target. For an operator already
using AD and an IdP, prefer AD-connected SSO. Consider direct LDAP only after an
operator confirms that running an IdP is an unacceptable burden and password-only
directory login meets their needs. This is a product judgment for
[self-hosters and small groups](../../PRODUCT.md), not a performance measurement.

| Route | Operator cost and trust boundary | Decision |
| --- | --- | --- |
| AD-connected OIDC | Register Sylux in an existing IdP; configure and maintain the IdP's directory bridge. Directory passwords go to the IdP, not Sylux. Provider MFA can stay at that boundary. New login depends on both services. | Preferred first route. A new IdP still adds deployment, database, backup and upgrade work; do not bundle one into Sylux. |
| Direct LDAP/AD | Configure a directory endpoint, CA trust, read-only lookup identity, schema and search policy. Sylux receives directory passwords and opens directory connections. No browser SSO or MFA assurance follows from a password bind. | Conditional later option. Qualify one directory profile before claiming support. No Kerberos, domain join or Windows integrated sign-in in this slice. |

Keycloak's [LDAP/AD federation guide](https://www.keycloak.org/docs/latest/server_admin/index.html#_ldap)
documents password validation at the directory and import/cache/edit choices.
For a disposable bridge, propose READ_ONLY edit mode and a directory identity with
no write rights. Inspect generated mappers, especially MSAD account state, and
cache settings. Provider login success does not establish that an AD disablement
revokes existing Sylux sessions. Measure that separately. Changing federation or
reimporting users must not silently relink accounts when OIDC subjects change.

Current [login code](../../src/http/auth.rs) looks up a local handle, checks its
password verifier and enabled flag, then rechecks account state and credential
epoch inside the session insertion transaction. [Account administration](../../src/http/accounts.rs)
handles local authority changes. The [dependency list](../../Cargo.toml) contains
no LDAP/OIDC client. These are source observations at baseline
`54bbaa39d08c7e1f944e7411d89c770b626129f2`, not live verification. A reverse proxy
or directory in front of the existing local login does not create external
account links.

[SYL-87](https://linear.app/corvus-dev/issue/SYL-87) owns directory connectivity,
lookup and authentication. [SYL-88](https://linear.app/corvus-dev/issue/SYL-88) owns
provisioning, linking, roles, revocation and file ownership. The contracts below
are inputs to SYL-88, not approved lifecycle policy. No directory writes,
password reset, synchronization or deployment is authorized here.

## Proposed direct connection and bind profile

The first candidate is **LDAPv3 simple name/password bind over validated LDAPS**
(default port 636). Require TLS 1.2 or newer, prefer TLS 1.3 where available, and
use current library security defaults. OpenLDAP documents both
[LDAPS and StartTLS](https://www.openldap.org/doc/admin26/security.html#Data%20Integrity%20and%20Confidentiality%20Protection).
StartTLS on port 389 is a later qualification candidate: require successful TLS
upgrade before any bind or search and never downgrade after failure. These are
proposed supported modes, not a Sylux compatibility claim.

[RFC 4513 sections 3 and 5.1](https://datatracker.ietf.org/doc/html/rfc4513#section-5.1)
requires confidentiality for password authentication and distinguishes anonymous,
unauthenticated and authenticated binds. Reject empty login names and passwords
before network access. A successful bind with an empty password can represent
anonymous authorization; it must never count as authenticated login.

Microsoft documents [LDAPS certificate requirements](https://learn.microsoft.com/en-us/troubleshoot/windows-server/active-directory/enable-ldap-over-ssl-3rd-certification-authority):
server authentication usage, DC DNS identity and a trusted CA chain. Validate
hostname, chain, validity and intended server usage on every new connection.
Allow a restricted private CA bundle for homelabs, with an explicit rotation
procedure; never expose a skip-verification setting or silently trust a fetched
certificate. OpenLDAP's [client TLS settings](https://www.openldap.org/doc/admin26/tls.html#Client%20Configuration)
provide the corresponding trust/verification controls. Qualify the eventual Rust
client's behavior rather than assuming OpenLDAP configuration applies to it.

Microsoft's [signing overview](https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/ldap-signing)
separates TLS from LDAP signing; signing alone does not encrypt passwords.
Its [channel binding guidance](https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/ldap-channel-binding)
says CBT applies to TLS with SASL, not simple binds over TLS, and recommends
Kerberos over TLS with CBT for strongest protection. Simple bind is a bounded
compatibility tradeoff that puts reusable passwords in Sylux's trust boundary.
If the operator requires SASL/Kerberos or stronger credential protection, use a
qualified IdP route or a separate design. Do not weaken DC signing/channel-binding
policies to make a fixture pass, and do not claim CBT support for this profile.

### Operator inputs and secrets

All fields here are proposed server configuration, not existing flags or UI.

| Input | Required behavior |
| --- | --- |
| Enablement and directory ID | Disabled by default. One explicitly selected directory, a permanent instance-local directory identifier and a separate directory login choice. Keep local administrator login independent. |
| Endpoint and trust | Explicit DNS host/port and LDAPS mode, operator-approved private destination and CA reference. Reject embedded URL credentials and unexpected URL parts. No login-supplied URLs, DNS discovery, automatic referrals or unapproved failover destinations. |
| Search bind | Dedicated least-privilege reader scoped to needed users/attributes; no domain administrator or directory write rights. Require a nonempty DN and secret. No anonymous search fallback. |
| Secret reference | Read from a server-side owner-restricted file or mounted secret outside served storage. Do not put secrets in tracked config, command arguments, URLs, web responses, telemetry or diagnostic exports. Define reload/rotation atomically; close old reader connections on replacement. |
| Search policy | Explicit base DN, scope, directory profile, allowlisted login attribute and fixed filter structure. Require stable-ID and enabled-state attributes to be readable. Reject invalid/incomplete configuration before enabling external login. |
| Resource limits | Bound DNS/connect/TLS, binds, searches, decoded result bytes, concurrency and total login time. Reject unknown configuration keys and impossible limits. |

Keep the user's password only for that authentication attempt; never save a local
verifier, cache successful passwords or include them in retries. Minimize copies,
clear secret buffers when supported, and review crash/core-dump and proxy/body
logging exposure. Bind secrets need an operator rotation procedure tested with
both old and new connections. The lookup account must not read password material.
A successful login does not require directory write access, though authentication
may update server-managed audit, lockout or last-logon state.

### Lookup and authentication sequence

1. Use existing CSRF protections and bounded login admission. Directory attempts
   need a separate bounded network-work budget so a hung directory cannot exhaust
   local administrator login capacity. Preserve password bytes, including spaces;
   specify input length limits independently of local password-creation rules.
2. Connect with validated TLS and bind as the lookup reader. Search only the
   configured base/scope with a server-built equality filter. Request just DN,
   stable ID, login/display fields and required account-state attributes.
3. Require exactly one complete successful search result, a valid stable ID and
   sufficient enabled-state data. Reject zero results, ambiguity, referrals
   (including `SearchResultReference` continuation references), partial results,
   size/time-limit errors or missing required attributes. Never accept the first row of a truncated search.
4. Bind on a **separate fresh TLS connection** using the returned DN and submitted
   password. Never rebind a pooled reader connection as a user. Close the user
   connection on every outcome. Base-read the returned DN through the reader
   before admission; require the original stable ID and current eligibility.
   Reject rename/replacement/state changes that prevent an unambiguous match.
   This narrows races but is not an atomic transaction with the directory.
5. Hand the verified `(directory ID, stable external ID)` to SYL-88's approved
   local-account policy. Recheck local account enablement/authority when creating
   the Sylux session, outside any directory I/O. Keep network calls outside SQL
   transactions. Authentication never grants a role by itself.

[RFC 4511 section 4.5.1](https://datatracker.ietf.org/doc/html/rfc4511#section-4.5.1)
defines base, single-level and subtree scopes plus search limits. Recommend one
explicit subtree under a dedicated people OU; use single-level if nested OUs are
not intended. The configured base/scope must exclude subordinate naming contexts.
Microsoft's [AD referrals documentation](https://learn.microsoft.com/en-us/windows/win32/ad/referrals)
describes subtree searches returning referrals to subordinate domains and
unexplored schema/configuration containers. A domain-root subtree search that
returns such continuation references is unsupported: fail the whole login even
if one matching user was returned. Do not chase or ignore the references; choose
a suitably scoped base, such as the dedicated people OU, and qualify it against
the target directory. This does not assume every AD domain-root search returns
references. Do not search an entire forest or global catalog by default.
[RFC 4515 section 3](https://datatracker.ietf.org/doc/html/rfc4515#section-3)
defines filter escaping. Use the client library's filter encoder, never string
interpolation of unescaped login text; DN escaping is a separate format. Attribute
names and filter operators come only from validated operator configuration.

Illustrative filters, not runnable Sylux configuration:

- OpenLDAP profile: `(&(objectClass=inetOrgPerson)(uid={escaped_login}))`.
- AD profile: `(&(objectCategory=person)(objectClass=user)(userPrincipalName={escaped_login}))`.

The placeholders mean encoded assertion values, not literal substitution syntax.
An operator must verify schema, ACLs and uniqueness in the chosen base. No broad
fallback across `uid`, email, UPN and short names after a failed search.

## Stable identity, eligibility and failures

| Concern | Proposed contract and evidence |
| --- | --- |
| LDAP identity | Use a directory namespace plus `entryUUID` when the directory supplies it. [RFC 4530](https://datatracker.ietf.org/doc/html/rfc4530#section-1) explains that DNs can be reused and UUIDs identify an entry after rename. Fail if absent; do not fall back to DN/email. Other directory schemas need a separately qualified immutable key. |
| AD identity | Use directory namespace plus `objectGUID`. Microsoft's [schema](https://learn.microsoft.com/en-us/windows/win32/adschema/a-objectguid) defines a system-set, unchangeable 16-byte value. Preserve binary identity with one tested encoding; do not confuse GUID byte order with string formatting. A new object with the same name must not inherit the old object's files. |
| Login names | Prefer full AD UPN; [Microsoft naming attributes](https://learn.microsoft.com/en-us/windows/win32/ad/naming-properties) distinguish it from `sAMAccountName` and stable GUID. A short-name profile must be explicitly domain-scoped and qualified. Names/email/DN are mutable lookup/display data. Define whitespace/case handling per profile and test against directory matching rules; do not invent global lowercasing or auto-link a matching local handle. |
| AD disablement | Read `userAccountControl`; Microsoft's [flag definition](https://learn.microsoft.com/en-us/troubleshoot/windows-server/active-directory/useraccountcontrol-manipulate-account-properties) identifies ACCOUNTDISABLE as mask `0x0002`. Reject disabled users even with a successful earlier bind. This bit is not a complete expiry/lockout/password policy check; user bind must also succeed. Expired/locked/must-change-password accounts fail closed, with operator guidance to the existing directory support route. No password-change flow here. |
| Generic LDAP disablement | LDAP schemas do not provide one universal AD-style enabled flag. Require a qualified schema-specific eligibility rule and negative tests; no production generic-directory claim based only on `inetOrgPerson` and bind success. Missing/unreadable eligibility data denies login. |
| Directory outage | Fail new external logins without local-password fallback for the same external identity. Never serve cached password success. Do not disable local administrators, erase account links or delete files. Existing sessions follow SYL-88's finite lifetime/revocation contract; directory bind alone cannot revoke an already issued Sylux session. |

Proposed initial bounds, to tune with fixture evidence: 3 seconds for
DNS/connect/TLS, 3 seconds per operation, 10 seconds total per login (including
all connections and rereads), two search entries maximum, 64 KiB decoded results
per login, and eight concurrent directory attempts with no unbounded queue.
Enforce client deadlines even when a server ignores requested limits. These are
Sylux design starting points, not protocol defaults or measured capacity.

Do not automatically retry user binds: retries can multiply lockout pressure.
Do not switch endpoint or authentication mode on failure. Future failover must
stay within the same qualified directory namespace and total deadline. For
invalid credentials, unknown/disabled/ambiguous users, return one generic login
failure without directory diagnostics or user enumeration. Infrastructure failure
may show a generic temporary-unavailable message only when it does not disclose
account existence. Operators need allowlisted failure categories (TLS, timeout,
reader configuration, policy, busy), never raw LDAP diagnostic strings, DNs,
login values, filters or passwords. Timing and rate-limit enumeration need tests.

Before enabling external login, verify a separate local administrator and the
[host recovery procedure](../docker-beta.md#recover-administrator-access).
It resets/enables an existing local administrator and revokes its sessions; it
does not create an account or reopen setup. Preserve backup verification and
service-stop requirements. Bad directory configuration should disable that login
route while local sign-in and host recovery remain usable. Never edit SQLite or
remove installation identity to recover access.

## Groups and SYL-88 handoff

Candidates only; none is needed for the first authentication qualification:

| Candidate | Value and cost | Recommendation to SYL-88 |
| --- | --- | --- |
| Explicit local roles | Small-group operators can approve individual account links and roles without recursive group queries. | Preferred first policy, pending SYL-88 approval. Never auto-merge local and external accounts. |
| Group-based admission | A dedicated group could restrict who may sign in. Adds group schema, membership, lookup ACL, removal latency and outage rules. | Assess only with a concrete operator need; fail closed on incomplete membership evidence. |
| Group-to-role mapping | Could reduce administration, but creates privilege escalation and stale authority risk. | Defer automatic mapping, especially administrator grants. Never equate AD Domain Admins with Sylux administrators. |

AD's [memberOf schema](https://learn.microsoft.com/en-us/windows/win32/adschema/a-memberof)
describes group DNs. It is not evidence of complete transitive authorization.
Before using groups, qualify direct versus nested membership, primary groups,
cycles, renamed/deleted groups, cross-domain membership, result limits and stable
group identifiers. Generic LDAP group schemas also need explicit qualification.
No group sync, broad directory browsing, writes, SCIM or background import is
proposed. If needed later, prefer admission before role automation.

SYL-88 must settle provisioning versus approved links, collisions, local passwords
on linked accounts, unlink/relink and directory replacement. Both direct LDAP
and OIDC need an agreed maximum upstream-disable-to-access-loss interval, local
session invalidation and stable private file ownership. A user can change status
between the final directory read and local session issuance; a finite revocation
contract must cover this distributed race. OIDC `(issuer, subject)` and LDAP
`(directory ID, stable ID)` are separate identity namespaces. Switching routes is
an explicit proof-and-link operation, never an email/name match. Deletion must
not transfer files to a recreated same-name identity.

## Disposable verification targets

**Planned only. No directory, provider, browser or recovery journey ran here.**

- Generic target: isolated OpenLDAP 2.6 fixture, exact patch/image digest selected
  at implementation, synthetic people OU, two users, duplicate-login entries in
  separate subtrees, read-only lookup identity, explicit stable IDs and a chosen
  disablement schema. Use trusted test TLS and a disposable CA. This proves only
  that profile, not all LDAP servers.
- AD target: isolated Windows Server 2025 AD DS VM on a private test network,
  exact edition/build/patch and lawful evaluation/licensing recorded before use.
  New test forest, no production trust or tenant, synthetic OU/users/groups,
  least-privilege reader and trusted DC certificate. Retain signing enforcement;
  record actual signing/CBT policy. OpenLDAP or a Samba substitute alone does not
  qualify Microsoft AD behavior. An available Windows Server 2022 fixture could
  be an additional target, never an inferred support claim.
- Bridge target: the same disposable AD through a pinned Keycloak version/digest,
  read-only federation, documented mappers/cache and a separate local IdP admin.
  Reuse SYL-86's OIDC profile and browser checks. Local Keycloak users alone prove
  nothing about AD disablement or federation caches.

Give fixtures no route to production directories; restrict LDAP ports to the
runner, use disposable Sylux storage and secrets, and destroy them after checks.
Record exact Sylux revision, versions, directory policy/ACL export, redacted
configuration, commands, test IDs and observations. Credentials and personal
identifiers must not appear in published evidence.

| Check | Required observation before claiming support |
| --- | --- |
| D1 Happy path | Real TLS search and user bind; approved A signs in, reloads and uploads/downloads a known fixture. Wrong and empty passwords fail without a session. Reader has no write/password-read rights. |
| D2 Transport/secrets | Reject untrusted, expired and wrong-host certificates; configured private CA succeeds. No cleartext credentials, silent downgrade or secret in logs/responses/export. Rotate reader secret and CA; old connections/secrets stop being used. If StartTLS is later added, strip/reject upgrade and confirm no bind occurs. |
| D3 Search boundary | Reject injection text, duplicate names, missing GUID/UUID/state, referrals, oversized/partial/time-limited searches and results outside configured scope. An AD domain-root subtree search returning `SearchResultReference` continuation references fails the whole login even with one matching user; a suitably scoped people-OU control without references succeeds. A valid escaped-name control succeeds. No referral is chased and no unapproved outbound connection occurs. |
| D4 Identity continuity | Rename/move A and change UPN/uid while preserving the stable ID: approved local account/files remain. Delete and recreate A with the same name: it cannot inherit files or the prior link. Test binary GUID conversion and same-name local/other-directory collisions. |
| D5 Policy and races | Disable, lock, expire and require password change in real AD; check fresh binds and admission. For LDAP test the selected eligibility schema. Hold authentication while locally disabling/demoting the account; issuance must honor current local authority. Measure upstream disablement with an existing session against SYL-88's bound. |
| D6 Failure capacity | Drop/delay DNS, TCP, TLS, search and bind; reject invalid reader secret and malformed replies. Check total deadlines, result/concurrency bounds, no bind retries and no cached success. Local administrator sign-in/logout still works under directory saturation and outage. |
| D7 Access isolation | A and B use separate browser contexts and different private files. Neither can list, download, replace, move, delete or recover the other's files, including guessed IDs. Unknown/ambiguous external identity and same-email local identity cannot acquire either account. |
| D8 Recovery | With directory and IdP down, exercise documented local administrator recovery on disposable data. Verify old sessions fail, setup stays closed and file checksums survive. Local logout stays independent of directory availability. |
| D9 AD-connected SSO | Repeat SYL-86 checks using real federated AD users; disable/delete/rename them in AD, vary Keycloak caches, and record new/existing session behavior. Compare with direct results without claiming automatic subject continuity or immediate revocation. |

For future account/session changes, use the existing
[authority verification](../../.agents/skills/verify-sylux-authority/SKILL.md) and
[storage journey selector](../../.agents/skills/verify-sylux-storage/SKILL.md),
then relevant TLS browser journeys and required CI. Directory tests alone do not
prove private storage isolation or recovery correctness.

## Unknowns and bounded follow-ups

Resolve actual operator constraints first: existing IdP, directory schema/version,
private CA ownership, read ACLs, required authentication policy/MFA, whether an IdP
is acceptable, availability of disposable AD, account-linking preference and
acceptable revocation delay. No exact server/client version is certified here.

Later issue candidates, not created or authorized by this document:

1. **Qualify the AD-to-Keycloak bridge first.** Pin the disposable AD/Keycloak
   targets, prove read-only access and D9 against SYL-86. Record disablement/cache
   limits as SYL-88 inputs. No production setup or automatic provisioning.
2. **Conditional direct LDAP feasibility.** Only with a confirmed operator need,
   select a maintained Rust client after advisory/license/toolchain review. Prove
   validated LDAPS, bounded search/bind, binary IDs and D1–D6 without issuing
   production accounts. Stop if required directory policy cannot be met.
3. **Conditional direct account integration.** Only after SYL-88 approves lifecycle
   rules, connect verified identity to local sessions and complete D1–D8, authority
   checks and required CI. No group role mapping in the first slice.

Defer StartTLS qualification, Kerberos/SASL, multiple directories/forest search,
automatic failover, password changes, sync and group automation until concrete
needs justify their separate verification cost. The research itself checked
primary sources, local source/links and documentation scope. It ran no application
builds, tests, containers, TLS binds or deployment; proposed checks are not results.
