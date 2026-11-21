## Change

Describe the user outcome and link the issue.

## Verification

Record relevant checks and any limits.

## Review

- [ ] Scope and acceptance criteria are met; exclusions are stated.
- [ ] For changes to specified [libraries and sharing](../docs/specs/libraries-and-sharing.md) behavior,
  check that each changed rule has a matching scenario and test, and that changed behavior tests match
  the rules. Mark planned scenarios separately from verified results. Refactors can cite existing passing coverage.
- [ ] UI changes meet the [required UI review rules](../DESIGN.md#required-ui-review-rules), with comparison evidence and an adoption decision recorded before implementation, or this PR has no UI change.
- [ ] Affected feedback lifetime, pending states and accessibility were checked, or are not affected.
