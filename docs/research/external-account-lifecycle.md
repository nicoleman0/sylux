# External account lifecycle proposal

[SYL-88](https://linear.app/corvus-dev/issue/SYL-88) decision proposal.
Evidence checked **18 November 2026** against Sylux
`511a42415e475d895b5cac16477e5ad0890bc239`.
No external identity support or lifecycle journey was implemented or executed.

## Recommendation and scope

Use explicit administrator-approved accounts and links for the first external
sign-in slice. Keep one external identity per ordinary Sylux account, local role
assignment and a separate, working local administrator. Identity removal blocks
access; it does not delete files or transfer their ownership. These choices fit
[PRODUCT.md](../../PRODUCT.md)'s small groups and occasional account administration.
They are proposed release requirements, not authorization to implement them.

This consolidates [SYL-86's OIDC proposal](sso-first-integration.md) and
[SYL-87's LDAP/AD proposal](ldap-ad-authentication.md):

- Start with one configured OIDC provider and disposable Keycloak qualification.
  Keep code flow, confidential client, S256, strict token/callback validation,
  bounded HTTPS and independent local logout from SYL-86.
- Prefer AD behind that IdP. Direct LDAP remains conditional on an operator who
  cannot accept the IdP cost. If selected, retain SYL-87's validated LDAPS,
  read-only lookup, stable IDs, enabled-state checks and bounded search/bind.
- Neither authentication route approves an account or grants a Sylux role.
  OIDC subjects and directory IDs are separate namespaces, even for one person.
- Require a measured upstream revocation bound before claiming safe lifecycle
  support. A short Sylux session alone cannot bound repeated stale upstream login.

## Existing Sylux behavior

These are **source observations at the revision above**, not test results from
this research. Proposed behavior begins in the following section.

| Boundary | Current evidence and consequence |
| --- | --- |
| Account and ownership | [Schema](../../migrations/sqlite/0001_s1.sql) gives accounts an `account_id`, unique handle, `user` or `administrator` role, enabled flag and credential epoch. Entries reference `owner_account_id`. External names must never replace that owner key. |
| Local sign-in | [auth.rs](../../src/http/auth.rs), `login`, checks password and enabled state, then rechecks enabled state, expected epoch and verifier inside session insertion. A future external callback needs the equivalent current-state check after network I/O. |
| Session validity | [authz.rs](../../src/domain/authz.rs), `validate_session`, requires an enabled account, matching epoch, unrevoked session, and live idle/absolute deadlines. The idle and absolute constants are 30 minutes and 12 hours respectively; the idle deadline is never refreshed, so the effective lifetime is fixed at 30 minutes from login. These deadlines do not establish an upstream-disablement guarantee. |
| Authority change | [accounts.rs](../../src/http/accounts.rs), `change_authority`, authorizes the actor, guards the last enabled administrator, updates authority/epoch and revokes target sessions in one transaction. It rejects self-disable/self-demotion. Preserve this ordering. |
| Password reset and promotion | [accounts.rs](../../src/http/accounts.rs), `reset`, installs a password verifier for an enabled ordinary account; `change_authority` can promote an enabled account. The proposed first slice must reject both reset and promotion for external-mode accounts. |
| Storage admission | [coordinator.rs](../../src/domain/coordinator.rs), `admit_current`, `revalidate_current` and durable admission/publication, validates the session and principal again at relevant boundaries. Keep serving SQL on [SqliteWorkers](../../src/persistence/sqlite.rs); network I/O stays outside transactions. Uncertain mutations retain their existing reconciliation rules. |
| Private files | [Isolation contract](../../tests/contract/isolation.rs), `a03_unrelated_and_admin_route_matrix_denies_owner_private_state`, covers owner, unrelated user, administrator and anonymous routes. Administrator role is not permission to read another user's files. |
| Recovery and audit | [Recovery guide](../docker-beta.md#recover-administrator-access) and [recovery contract](../../tests/integration/admin_recovery.rs) reset/enable an existing administrator and revoke its sessions without reopening setup or changing other accounts/files. The schema's current `event` table stores actor, action, time and outcome; it is not a complete identity audit trail. |

The [bounded authority model](../../verification/account-authority/README.md)
covers two administrators disabling/demoting each other. It does not model
external callbacks, expiry, provider events, recovery or distributed directory
races. Future changes need reviewed source correspondence and the real HTTP
contract through the [authority skill](../../.agents/skills/verify-sylux-authority/SKILL.md).

## Proposed identity and admission contract

### Stable keys and collisions

| Route | Protocol fact | Sylux proposal |
| --- | --- | --- |
| OIDC | [Core section 5.7](https://openid.net/specs/openid-connect-core-1_0.html#ClaimStability) identifies `iss` plus `sub` as the stable key, unlike email/username. [Subject types](https://openid.net/specs/openid-connect-core-1_0.html#SubjectIDTypes) include pairwise identifiers. | Persist the exact validated issuer and case-sensitive subject. No lowercasing, URL rewriting or email-based merge. Client/sector, realm or issuer replacement requires explicit requalification and relinking if the key changes. |
| LDAP | [RFC 4530 section 1](https://www.rfc-editor.org/rfc/rfc4530.html#section-1) explains that DNs can be reused and `entryUUID` locates an entry after rename. | Use permanent operator-assigned directory namespace plus `entryUUID`, only for a qualified schema. Missing ID denies login; DN, login and email are lookup/display data. |
| AD | [Microsoft objectGUID schema](https://learn.microsoft.com/en-us/windows/win32/adschema/a-objectguid) defines a system-set, unchangeable 16-byte identifier. | Use directory namespace plus `objectGUID`, with one tested binary encoding. Rename/move preserves the link; delete/recreate does not. Directory replacement gets a new namespace even if its DNS name is reused. |

Require uniqueness of the external key and of the account's active link in the
same local transaction. Concurrent claims must yield one success and a conflict,
never overwrite a link. Reserve detached/revoked keys against automatic reuse;
reassignment to another local account is excluded from the first slice. A
provider violating subject non-reassignment is a trust failure requiring operator
intervention, not something email comparison can repair.

Matching email, `email_verified`, handle, DN, UPN or group name never proves
ownership of an existing Sylux account. Reject a chosen local handle collision
without changing the existing account; an administrator selects another handle.
Do not expose whether an unknown external identity matches a local account.

### Explicit provisioning and linking

Propose this bounded approval flow, with no public registration or automatic JIT
account creation:

1. An administrator creates a pending approval for an exact external key and a
   new ordinary account or a specific existing ordinary account. Obtain that key
   from a trusted provider administration channel, not an email match or a
   claimant-supplied profile. Confirm the intended person through the operator's
   existing trusted contact process. Pending records confer no storage access.
2. The intended person proves the external identity through a fresh, validated
   SYL-86 callback or SYL-87 bind. The key must exactly match the pending approval.
   For an existing local account, also require recent local-password proof tied
   to that same browser and account. Administrator approval alone cannot silently
   attach an identity to an existing user's private files. This protects against
   mistaken or claimant-driven links, not a malicious administrator: today an
   administrator can reset an ordinary user's password and supply that proof.
3. Use a single-use, browser-bound approval transaction, proposed five-minute
   lifetime, bounded count and no identity/secret in its URL. Recheck the approving
   administrator's current authority, target state, link uniqueness and approval
   version at commit. Capture account epoch and link generation before external
   proof; require them unchanged at issuance, including disable/re-enable races.
   A demoted approver, expired/replayed approval or disabled existing target fails
   closed. A new account activates only through this approved path.
4. Atomically activate the link, retain the immutable local `account_id`, remove
   the ordinary account's local password verifier, increment its epoch and revoke
   old sessions. Issue a fresh external-origin session only after current-state
   checks. Make the credential conversion explicit before confirmation.

This is **explicit provisioning with proof at activation**, not JIT admission.
Unknown valid external users receive no Sylux account/session. No provider token
or directory password becomes a local password. An ordinary external account has
no local-password fallback, including during outage; otherwise upstream removal
would leave a second access path. Lost local proof for an existing account needs
a separate reviewed recovery decision, not weaker linking. Local administrator
accounts cannot be linked or converted in the first slice.

For implementation, distinguish local and external credential modes explicitly;
a nullable password field alone is not evidence of the intended policy. The existing
`POST /accounts/{id}/reset` and role-change promotion route must enforce the
external-mode rejections above. Store
session origin/link generation so link revocation cannot leave sessions live.
This describes constraints, not a migration/schema design.

### Roles, disablement and file ownership

Roles remain local. An approved new external account starts as `user`. Ignore
provider roles/groups for authorization. Explicit local promotion may be assessed
later; the first slice keeps administrators local so provider failure cannot
remove the recovery administrator. Never map Domain Admins or a group named
`admin` to Sylux administration.

| Transition | Proposed access effect | Ownership and return path |
| --- | --- | --- |
| Local disable | In the existing serialized authority boundary, disable the account, advance epoch and revoke all sessions. Reject held callbacks/approvals at issuance. | Preserve account ID, links, files and Trash. Local enable is deliberate and cannot revive old cookies or bypass fresh external proof. |
| Upstream disable/delete | Fresh authentication must fail once upstream eligibility converges; existing access ends within the qualified bound below. For urgent removal, the operator also disables the account locally. | No implicit delete, unlink or file cleanup. Successful future upstream login cannot override local disablement. |
| Deprovision | Administrator explicitly disables locally and revokes the link/approvals, advancing epoch and revoking sessions atomically. No background deletion in this slice. | Keep disabled account and owned data. Same-name recreation receives no old link or files. Restoration requires explicit approval of the same key/account, not automatic sync. |
| Unlink or provider removal | Revoke affected links, pending approvals and sessions; disable accounts left without credentials. No resurrection of an old local verifier. Provider configuration replacement must not reuse revoked links silently. | Keep ownership. First slice has no self-service unlink for an external-only account. A later local conversion must verify identity, install a new credential and deliberately enable access. |
| Rename/relink | Name changes with the same key retain account identity. Different keys require a new proof-and-approval process, including proof of the existing account's control; external-only relinking is deferred. | No email-based account merge, automatic cross-protocol migration, ownership transfer or hard account deletion. |

The first slice does not add administrator browsing/export of another account's
files. Retention and eventual erasure are separate operator decisions; do not
promise indefinite preservation as a backup policy. Retained disabled-account
data continues to use storage. No identity event may silently purge it.

## Proposed session and revocation bounds

Protocol facts: OIDC [authentication requests](https://openid.net/specs/openid-connect-core-1_0.html#AuthRequest)
define `max_age` and returned `auth_time`; this is authentication freshness, not a
directory-state guarantee. [Back-channel logout sections 2.4–2.7](https://openid.net/specs/openid-connect-backchannel-1_0.html#Backchannel)
define signed logout messages and RP processing. Neither ensures that every
provider emits an event on account disablement. [RP-initiated logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html#RPLogout)
is a separate provider sign-out request.

Proposed first qualification envelope, **not a measured guarantee**:

- External access expires at a fixed deadline no later than **15 minutes from
  the start of fresh upstream authentication**. Activity and login retries cannot
  slide it. Keep the idle limit no greater than that deadline. Do not reuse the
  current 12-hour local absolute constant for external sessions; the current
  non-sliding idle deadline already limits local sessions to 30 minutes.
- Renew only with new upstream proof and current local admission checks. OIDC
  requests fresh authentication (`max_age=0`), validates `auth_time` within the
  bounded attempt, and qualifies provider eligibility enforcement. Direct LDAP
  repeats the qualified bind and stable-ID/enabled-state read. No refresh/offline
  tokens or cached password success extend access. A delayed callback must not
  start a fresh 15-minute clock from its eventual arrival. Use the server-recorded
  attempt start to cap access; accepted token clock tolerance cannot extend it.
- Let **P** be the qualified worst-case delay from authoritative upstream
  disablement to denial of *all* new authentication, including replication,
  federation caches, alternative authentication paths, existing IdP SSO and
  authentication already in flight.
  Propose **P ≤ 5 minutes** and therefore **P + 15 ≤ 20 minutes** to loss of
  new authorized requests. Operator acceptance and fixture evidence are required.
  A successful login just before P is the worst case to test.
- If P cannot be bounded, there is **no finite upstream revocation claim**.
  Do not enable the proposed profile with that claim. Ask whether the operator
  accepts explicit local disablement as the authoritative removal procedure, or
  requires a separately qualified event/polling mechanism. Record that choice
  before implementation; do not silently substitute it for upstream removal.

Local disable, role/credential change and link revocation take effect for the
next authorization decision after their transaction commits; already admitted
requests and bytes already delivered cannot be recalled. Preserve storage
revalidation at publication. Future verification must measure reads/downloads
already in progress and specify cancellation limits before calling the envelope
complete. The 20-minute target bounds new authorized requests, not recall of an
open download or proof of an atomic transaction across Sylux and a directory.

During provider/directory outage, deny new external proofs and renewals. Existing
external sessions may operate only until their fixed deadline unless locally
revoked earlier. Do not extend deadlines, erase links/files, change roles or
fall back to local credentials. Keep local administrator login/logout and host
recovery usable under external connection saturation. Configuration errors must
isolate the external route rather than prevent Sylux startup/local recovery.

Local logout revokes the Sylux session regardless of provider availability.
Optional provider logout follows SYL-86's separate consent/result handling and
logout-only token-hint privacy exception. Failure cannot undo local logout.
It is not evidence of global logout or upstream disable propagation.

## Related feature decisions

Each decision is a **proposal for later approval**. “Needed” means necessary for
the selected slice, not permission to implement now. Costs are design estimates,
not measured performance or staffing estimates.

| Candidate | Need and value | Cost and boundary | Decision |
| --- | --- | --- | --- |
| Explicit approvals and local roles | Small groups need known users without accidental file takeover. | Manual approval/proof, local transaction/race tests and occasional administration. | Needed for first slice; use the contract above. |
| Open JIT provisioning | Useful where many users join without an administrator. No confirmed operator volume requires it. | Admission policy, quota abuse, handle collisions and cleanup for unwanted accounts. | Defer. Do not create a backlog item until join volume makes explicit approval inadequate. |
| Provider MFA | An operator may require stronger sign-in without Sylux managing factors. | Provider policy/enrolment/recovery, freshness and assurance qualification. Direct password bind supplies no equivalent assurance. | Use provider enforcement for the OIDC target; require proof of policy if MFA is an operator requirement. No native Sylux factor store in this slice. |
| SCIM provisioning | Useful for managed join/leave automation at larger scale; no confirmed small-group need. | Authenticated management API, token rotation, schema/patch/idempotency, races, retention and last-admin safeguards. | Defer. [RFC 7643 section 4.1.1](https://www.rfc-editor.org/rfc/rfc7643.html#section-4.1.1) defines `active` as administrative status; [RFC 7644 section 3.6](https://www.rfc-editor.org/rfc/rfc7644.html#section-3.6) defines DELETE visibility, not Sylux file erasure. Future SCIM semantics must preserve this ownership contract. |
| Group admission/synchronization | Could let an operator restrict sign-in to a known group. | Stable group IDs, direct/nested/primary membership, cycles, truncation, removals, cache delay and outage policy. [AD memberOf](https://learn.microsoft.com/en-us/windows/win32/adschema/a-memberof) describes group DNs; it does not establish complete transitive authorization. | Defer until a named group-admission need exists; prefer admission before role automation. No sync daemon now. |
| Group-to-role mapping | Could reduce manual role work, but offers little for occasional administration. | Escalation risk, stale privilege, conflicting mappings and recovery constraints. | Defer, especially administrator grants. Never infer privilege from group display names. |
| Provider logout events | Could shorten access after provider logout. | Signed-token validation, issuer/audience/session mapping, replay handling, delivery loss and disable-event qualification. | Conditional only if the accepted revocation envelope needs it. Keep finite expiry as a backstop; events alone are not reliable delivery proof. |
| Identity audit events | Operators need to explain account approval, access loss and recovery. | Minimal event schema, restricted access, retention, redaction and failure-path tests. | Needed alongside a future slice; no audit dashboard/SIEM/export platform. |

OIDC [ID Token claims](https://openid.net/specs/openid-connect-core-1_0.html#IDToken)
include optional `acr`/`amr`; their presence alone is not a universal MFA guarantee.
[Keycloak authentication flows](https://www.keycloak.org/docs/latest/server_admin/index.html#_authentication-flows)
show conditional versus required factors. Require the operator to name the
required policy and, if Sylux must enforce an assurance level, qualify an exact
provider-specific claim policy and reject missing/insufficient assurance. A
password-only path or enrolment bypass must fail the MFA acceptance journey.
Local recovery remains a separate trusted-host boundary; document it as such.

Audit proposal: allowlist approval/link activation/revocation, account
provision/disable/enable, local role change, external session issuance/revocation,
recovery and categorized authentication failure events. Record time, action,
outcome and only the internal actor/target/link references needed for restricted
operator review. Anonymous failures use categories and bounded counters, without
raw external identities. The current event schema lacks target/link fields;
any extension needs review, not a claim of existing audit coverage. Make successful
administrative effects and their audit entry atomic; fail explicitly if that
write fails. Bound failure-event volume so authentication abuse cannot fill disk.

Keep credentials, tokens, assertion bodies, email, DN, group lists, file names,
paths and raw provider/directory errors out of logs, telemetry and exported
evidence. Internal references remain sensitive. Restrict audit access to local
administrators, document retention/pruning before shipping, and never mirror
identity records into the redacted [contract-event stream](../../src/persistence/mod.rs).
No user activity surveillance, directory import inventory or compliance claim.

## Local administrator recovery requirements

Before enabling external login, prove an existing independent local administrator
can sign in, and rehearse the [host recovery guide](../docker-beta.md#recover-administrator-access)
on disposable storage. Preserve an enabled local administrator, not merely an
externally dependent administrator. Future promotion/link/provider-removal paths
must enforce that stronger recovery condition in the same authority transaction.

Do not link the recovery account, auto-demote it through groups, or let provider
configuration disable its local route. Recovery still requires trusted host
access, a stopped service and verified backup; it resets/enables an existing
administrator and revokes its sessions. It does not create an account, reopen
setup, repair SQLite or transfer private files. Never edit SQLite or installation
identity as an identity-outage workaround.

## Disposable future verification journeys

**All journeys below are planned and unexecuted.** Source and link review is the
only verification completed by this research. No provider, directory, browser,
build, authority model, storage test or recovery command ran.

Use disposable Sylux storage, a separate local administrator R, two ordinary
synthetic identities A/B, separate browser contexts and trusted test TLS. Pin the
Sylux revision, Keycloak image/digest, policies, cache settings and test CA. For
AD, reuse [SYL-98](https://linear.app/corvus-dev/issue/SYL-98)'s isolated real AD DS
fixture and read-only bridge; local Keycloak users cannot qualify AD lifecycle.
Use no production tenant, directory, files or credentials. Capture redacted
commands, versions, deadlines, request outcomes, checksums and cleanup evidence.

| ID | Future action | Expected result |
| --- | --- | --- |
| L1 Approved sign-in | Approve A's exact key; authenticate via actual Keycloak, reload, upload and download a known fixture. Try unknown and same-email keys. | Only approved A gets its account; exact file bytes survive reload. Unknown/collision cases create no account/session. Repeat SYL-86 V1/V3/V4 callback, replay and transport controls. |
| L2 Link and continuity | Give A an existing local account/file; approve conversion, prove both credentials and race two activation callbacks. Rename upstream, then delete/recreate the same name. Try administrator reset of converted A, B's browser/proof, reused approval, changed issuer and conflicting link. | One activation, same account/owner ID and bytes; old local password/cookies fail. Administrator reset of converted A is rejected and installs no verifier. All mismatches deny without reassignment. Recreated identity cannot inherit A's files. Test GUID/UUID encoding if direct LDAP is approved. |
| L3 Local disable race | Hold token exchange or LDAP bind after proof starts. Disable A locally, then release it. Also race disable versus session insertion, disable/re-enable during proof and approver demotion versus link commit in both orders. | Disabled state wins at the next authorization boundary; no usable issued session. Earlier sessions are revoked. Demoted approver cannot activate the link. No partial account/link effect or false success audit event on rejection. |
| L4 Upstream disable and outage | Disable A upstream at recorded time while A stays active; attempt fresh authentication repeatedly, including an attempt just before P, a held callback and existing IdP SSO. Stop/delay provider/directory, vary bridge caches, alternative authentication paths, AD replication and directory controller/site selection. | Measure P and final new-request denial against P + 15 minutes, proposed ≤20. No renewal during outage or sliding deadline. If stale fresh proofs continue beyond P, qualification fails. Inspect open downloads separately and state cancellation limits. Check a resumable upload spanning expiry and resumption under a fresh session. |
| L5 Deprovision/unlink | Revoke A's link, remove provider configuration and re-enable A locally without relinking. Try old sessions, pending approvals, old password, administrator reset and same-name replacement identity. | Reset is rejected and installs no verifier; no access revival. Files/Trash/checksums and owner ID remain; B receives nothing. Explicit approved restoration of the same key requires fresh proof. |
| L6 Recovery and last admin | Make external services unavailable/saturated. Use R's local login/logout; rehearse documented host recovery after stop/verified backup. Attempt to promote external A; concurrently disable/demote two local admins through real HTTP. | Local paths stay usable; reset invalidates old password/cookies, keeps setup closed and files unchanged. Promotion of external A is rejected; the authority race preserves an enabled administrator and rejects revoked actors. Future external paths also preserve a local recovery administrator. |
| L7 Two-user isolation | A/B each create private live and trashed files. Try guessed entry/revision/upload/mutation/removal IDs across list, download/preview, replace, move, trash, restore, retry and reconcile routes, also as R. | Neither user nor R can read/mutate the other's private state; use existing isolation contract's status/privacy expectations. Each owner control succeeds with exact bytes. Repeat after rename, link activation, revocation and reapproval. |
| L8 MFA/logout/audit | Require provider MFA, attempt password-only/unenrolled and insufficient/missing assurance paths; sign out with provider up/down. Inspect restricted audit, app/proxy logs and responses, including failure floods. | Required policy cannot be bypassed; local logout always rejects old cookie. Provider logout reported separately. Events are complete/minimal, no credentials or raw identities leak, and bounded failure logging preserves local availability. |

For future implementation, run the authority skill's model and exact HTTP
contract, then the [storage journey selector](../../.agents/skills/verify-sylux-storage/SKILL.md)
and relevant real TLS browser checks. Existing test fixtures require extensions
for external identity; their present existence is not passing evidence for L1–L8.
Keep SYL-86 V1–V8 and SYL-87 D1–D9 as route-specific inputs rather than replacing
those security checks with successful sign-in alone.

## Operator unknowns and bounded follow-ups

Unanswered inputs before implementation approval:

- Existing IdP/directory versions and topology, private CA ownership, who rotates
  secrets, available disposable AD licensing/fixture, and whether an IdP is an
  acceptable operating burden.
- Join/leave frequency, whether explicit provisioning and exclusive external
  credentials fit, and who can verify a person's identity for linking/recovery.
- Accepted upstream-disablement delay, whether the proposed 20-minute envelope
  and frequent reauthentication are tolerable, directory replication/cache bounds,
  directory controller/site selection, alternative authentication and cache paths,
  and the required treatment of already-open downloads. No bound is qualified yet.
- Required MFA policy/assurance, local administrator custody, recovery rehearsals,
  audit retention and disabled-account storage/erasure policy.

Live Linear review on the checked date found the related
[SYL-98 bridge qualification](https://linear.app/corvus-dev/issue/SYL-98) already in
Backlog. Its full scope includes pinned fixtures, connector write rejection,
subject continuity and account-state/cache/outage measurement. Reuse it for P;
do not create a duplicate bridge issue or imply this backlog item authorizes a run.
Searches for OIDC, identity, SCIM, provisioning and groups informed the following
recommendations; none was created or authorized by this record:

1. **One-provider disposable qualification**, as already proposed in SYL-86:
   after operator/profile selection, qualify strict OIDC and fresh eligibility,
   subject continuity and assurance using real Keycloak plus fault controls.
   Feed AD-specific results from SYL-98 into it. No production accounts or JIT.
2. **Approved-link account/session slice**, only after lifecycle and operator
   choices are approved: ordinary accounts, exact keys, explicit proof/approval,
   local roles, revocation/expiry, minimal audit and independent local recovery.
   Acceptance is L1–L8 plus relevant authority/storage/route checks. If this is too
   large for one delivery issue, split at reviewed interfaces, not into extra
   feature candidates. Include external logout from SYL-86 only within its scope.
3. **Conditional revocation mechanism**, only if measured P or the accepted
   removal interval makes the first profile insufficient: qualify one named
   provider's disable-to-event or bounded eligibility check, including delivery
   loss/outage and replay. No generic event platform or group sync. Direct LDAP
   feasibility/integration remains SYL-87's conditional candidate, not another
   required workstream.

Do not file SCIM, JIT, group-role automation, multiple providers, SAML, directory
writes, ownership transfer or native MFA just to complete an identity checklist.

## Evidence and drift limits

Primary passages and their cited anchors were reviewed for the claims above,
including OIDC stable keys/freshness/assurance, directory UUID/GUID, SCIM status
and DELETE semantics, provider logout, Keycloak policy/federation and AD group DNs.
Local links, source correspondence and the docs-only diff were checked.

The date records retrieval, not vendor support certification. Keycloak `latest`,
Microsoft guidance, operator policy, provider metadata and cache behavior can
change; recheck them against pinned versions and actual fixtures before building
or shipping. Local behavior is limited to the stated commit. No application test,
conformance, security guarantee, deployment or durability qualification follows
from documentation research or the planned journeys.
