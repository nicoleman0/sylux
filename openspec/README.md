# OpenSpec pilot

[SYL-117](https://linear.app/corvus-dev/issue/SYL-117/pilot-openspec-for-sylux-capability-rules-and-library-changes)
pilots OpenSpec for private storage. Linear owns scope, priority, issue
relationships and work state. Use BMAD to explore unresolved design questions.
OpenSpec records current behavior in `specs/` and proposed deltas in
`changes/`. The [deferred sharing research](../docs/research/library-sharing.md)
has the candidates and open decisions for subsequent issues.

## First use

From the repository root, use the Node version in [`.nvmrc`](../.nvmrc)
and run the pinned [OpenSpec CLI](https://github.com/Fission-AI/OpenSpec/releases/tag/v2.1.1).
The tracked files are ready to use. Setup is one shell function and a temporary
npm cache.

```sh
openspec_cache=$(mktemp -d)
openspec() {
  OPENSPEC_TELEMETRY=0 npm_config_cache="$openspec_cache" \
    npx --yes --package=@fission-ai/openspec@2.1.1 -- openspec "$@"
}
openspec --version
openspec list --specs
openspec show private-storage --type spec
openspec list
openspec show add-private-libraries --type change --deltas-only --json
openspec validate --all --strict --no-interactive
```

- **Current rule.** Start with [account isolation](specs/private-storage/spec.md#requirement-account-isolation-for-private-files-and-folders)
  and its linked HTTP check.
- **Proposed change.** Read the [SYL-110 proposal](changes/add-private-libraries/proposal.md),
  its [pending migration decision](changes/add-private-libraries/design.md#decisions)
  and the [delta](changes/add-private-libraries/specs/private-storage/spec.md).
- **Behavior checks.** Follow the [implementation tasks](changes/add-private-libraries/tasks.md)
  for LIB-01 and LIB-02. Use the existing [browser navigation](../web/tests/browsing-context.spec.ts),
  [keyboard](../web/tests/list-keyboard.spec.ts), [HTTP isolation](../tests/contract/isolation.rs),
  [migration](../tests/integration/schema.rs) and [single-file link](../tests/contract/content.rs)
  suites as starting points. The implementation PR records the new journey results.

## Propose and verify a change

1. Start from the relevant Linear issue and current rule. For SYL-110, use
   the active `add-private-libraries` change. To start another scoped change,
   run `openspec new change <change-id>`. Use the pilot's proposal, spec
   delta and tasks as examples. Add a design when it records how to resolve
   an implementation decision.
2. Put the behavior delta in `changes/<change-id>/specs/<capability>/spec.md`.
   Give each changed requirement a scenario and name the corresponding
   behavior test or where it will be added. Run
   `openspec validate <change-id> --type change --strict --no-interactive`
   before review. Validation checks the documents. Run the behavior tests
   during implementation.
3. Once the implementation and its checks have passed review, confirm all
   tasks are complete and run `openspec archive <change-id>`. Review the
   merged current spec and the dated directory in `changes/archive/`, then
   run `openspec validate --all --strict --no-interactive` and
   `openspec validate --archived --no-interactive`. For an archive rehearsal,
   copy the checkout to a disposable directory first. When archiving SYL-110,
   update the first-use example and links to reflect the archive.
