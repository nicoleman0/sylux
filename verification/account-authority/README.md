# Account authority race

This bounded model checks two enabled administrators, A and B. Each submits
one request to disable or demote the other. The initial states cover all four
operation pairs. The scheduler covers both transaction orders.

`positive.cfg` follows the current source: current authorization, the
last-admin count guard, account mutation and target-session revocation form
one atomic transaction. The successful request returns 204. The other request
then returns 401 because its session has been revoked. Rejected requests
change no account or session state.

`negative.cfg` deliberately caches **both** authorization and the admin-count
decision before applying either effect. Both requests can cache 204, remove
each other's authority and leave no enabled administrator. This is a changed
protocol used to check that the invariant detects the race. It is not a
reproduction of a bug in the current code.

Moving only the count check outside the transaction does not establish this
counterexample while current authorization stays inside it. In this scope,
the authorized actor is itself an enabled administrator and cannot remove
itself. After the first change, the second actor cannot authorize. The count
guard is defense in depth for these requests.

## Rules and bounds

- `TypeOK`: accounts, sessions, requests and results remain in their finite domains.
- `AtLeastOneAdmin`: at least one enabled administrator remains.
- `NoSuccessfulRevokedAuthority`: no successful effect has used an actor whose
  session or enabled-admin authority was no longer current at the effect.
  A history flag records this even if later states could hide it.
- `CompletedRaceOutcome`: when both requests finish in the correct protocol,
  exactly one enabled admin remains and the results are 204 and 401.
- Deadlock checking remains enabled. `Done` is a terminal no-op after both
  requests finish; an unfinished request must have a next step.

The positive config checks every rule above. The negative config checks
`TypeOK` and `AtLeastOneAdmin`, so the expected reported failure is the loss
of the last admin. It does not ask TLC to check the history or final-result
rule. No fairness or eventual-completion property is claimed.

## Source mapping

| Model element | Owning source |
| --- | --- |
| `Authorized` and `Decision` | `src/http/accounts.rs`: `authorize` and `change_authority`; current session/role check and last-admin count guard |
| `sessionLive` | `src/domain/authz.rs`: `validate_session`; abstracts revocation, account enabled state and matching credential epoch for the original valid session |
| `Finish` | `src/http/accounts.rs`: `change_authority`; account update, epoch increment and session revocation; audit insertion is not represented |
| Atomic `Execute` | `src/persistence/sqlite.rs`: `SqliteWorkers::transact_result` and `run_result_transaction`; writer submission and immediate transaction enclose the operation |
| Four initial operation pairs, 204/401 results and one remaining admin | `tests/contract/setup_auth.rs`: `concurrent_account_authority_changes_preserve_an_enabled_admin_and_reject_revoked_actor` |

The contract test also tries further actions with the losing session and
checks no extra account changes or audit events. Those follow-up requests
are outside this two-request model. The model explores both serial orders;
one concurrent test run need not execute both orders.

`coverage.json` records the reviewed base commit and whole-file SHA-256 hashes
for these four sources. A mismatch requires reviewing the protocol, model
and contract linkage. Do not refresh a hash merely to make a check pass.

## Reproduce

Use Python 3 and Java 11 or newer. The runner checks source correspondence,
verifies the TLC pin in [`../tlc.json`](../tlc.json), and checks both configs.
It does not require a globally installed agent skill. From the repository root:

```sh
evidence=$(mktemp -d)
python3 scripts/check-account-authority.py \
  --jar "${TMPDIR:-/tmp}/sylux-tla2tools.jar" --download \
  --output "$evidence/check"
```

`--download` explicitly fetches the pinned JAR only if missing. An existing
JAR must have the expected checksum. Set `JAVA` to select a Java executable.
Use `TLA2TOOLS` instead of `--jar` when reusing an installed JAR. Evidence
contains both TLC logs and a summary; the output directory must be new.
The CI job runs the same command and retains its logs as an artifact.

The positive run should complete with no error. The negative run should exit
12 and report `Invariant AtLeastOneAdmin is violated`. A shortest bad path is:
A caches both checks; B caches both checks; A removes B; B removes A using its
cached decision. The opposite apply order is possible too.

The optional installed `pstack-statechart` launcher can use `view.mjs` with
these configs to draw the graph. Its output directory must also be outside
the checkout. The view uses plain labels and merges request kinds where the
observable authority state is the same. It hides transitions that stay in
one chart label, including terminal no-ops and some request bookkeeping;
TLC still checks those transitions.

## Limits

- Two accounts, one original valid session per account, one request per actor,
  and only cross-account disable/demote requests. No full account universe,
  promotions, re-enabling, password resets or self-change paths.
- Session validity is a Boolean abstraction. No tokens, passwords, CSRF,
  credential-epoch overflow, new login, expiry or idle-time extension.
- No setup or offline recovery, worker shutdown or transaction failure paths.
- No SQL, filesystem or power-loss durability claim. Serialization and atomic
  success/rollback are assumptions taken from the reviewed implementation.
- This checks the stated model and invariants. It is not a proof of Rust code,
  database behavior, upgrade safety or deployment qualification.
