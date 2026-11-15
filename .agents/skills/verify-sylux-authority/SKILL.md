---
name: verify-sylux-authority
description: Verify Sylux's concurrent administrator disable/demotion rule when changing account authority, session validation or SQLite transaction boundaries. Run the bounded model and real HTTP contract; review source correspondence when either changes.
---

# Verify Sylux account authority

The bounded model checks two administrators trying to disable or demote each other.
It must preserve an enabled administrator and reject the actor whose session the
first transaction revoked. Read the [source map and limits](../../../verification/account-authority/README.md)
before interpreting a result.

## Run

Use the repository's declared Rust/Node/npm versions. Keep `web/dist` current before
Cargo checks; install dependencies on a fresh checkout or after toolchain changes.

The model runner needs Python 3 and Java 11+. It verifies the TLC checksum from
[`verification/tlc.json`](../../../verification/tlc.json). This command explicitly
downloads the pinned JAR if missing and retains evidence outside the checkout:

```sh
model_evidence="$(mktemp -d "${TMPDIR:-/tmp}/sylux-authority.XXXXXXXX")"
python3 scripts/check-account-authority.py \
  --jar "${TMPDIR:-/tmp}/sylux-tla2tools.jar" --download \
  --output "$model_evidence/check"
```

Alternatively, set `TLA2TOOLS` to an existing verified JAR and omit `--jar` and
`--download`. `JAVA` selects an explicit Java executable. A missing or invalid
runtime is a failed check, not a model pass.

Link the model to the real HTTP implementation:

```sh
cargo test --locked --features test-harness --test contract_setup_auth \
  concurrent_account_authority_changes_preserve_an_enabled_admin_and_reject_revoked_actor -- --exact
```

The test starts disposable local fixtures and exercises all four disable/demote
pairs. It must run one test, not zero. Successful paths close servers and workers
before deleting fixture roots. Preserve logs separately; failed fixtures may need
inspection. No deployed instance or real account is used.

## Review correspondence

The runner refuses changes to the source files recorded in
[`coverage.json`](../../../verification/account-authority/coverage.json). Compare
changed code with the mapped actions, including uncommitted changes. Update the
model if transaction boundaries or authority rules changed; preserve the promised
invariant unless a product decision explicitly changes it. After reviewing the mapping, record candidate hashes and the source commit (or
explicit uncommitted input state), then run the linked HTTP contract and both
model configurations. Keep the coverage update only when those checks pass. Full-file fingerprints are conservative review prompts, not an
automatic proof that the model describes the source.

The positive configuration must complete without errors. The negative control
must violate `AtLeastOneAdmin`; a parser error or a passing negative control fails
the runner. Its stale checks deliberately move both authorization and the count
guard outside the mutation. Moving only the count query is not this counterexample.

For a visual explanation, use the installed `pstack-statechart` skill with this
model, the selected config and `view.mjs`. The CI runner uses plain TLC and does
not require that global skill. A model counterexample is hypothetical until its
steps can be replayed against code. Do not present the intentionally broken model
as a product bug, or a passing model as deployment or durability qualification.
