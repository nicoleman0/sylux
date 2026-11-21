## 1. Private library creation and browsing

- [ ] 1.1 Implement library identity and owner-only creation, listing and
  lookup, and add the My libraries navigation.
- [ ] 1.2 Verify LIB-01. Extend the browser journeys in
  `web/tests/browsing-context.spec.ts` and `web/tests/list-keyboard.spec.ts`
  for library identity, reload, saved URLs and keyboard navigation. Extend
  `tests/contract/isolation.rs` for cross-account listings, search and direct
  requests against the real server.

## 2. Migration and compatibility

- [ ] 2.1 Record Nick's migration decision in SYL-110, update the delta as
  needed and implement the upgrade path.
- [ ] 2.2 Verify LIB-02 with a pre-upgrade database fixture. Use
  `tests/integration/schema.rs` for the upgrade harness and
  `tests/contract/content.rs` for the owner and signed-out link checks.
  Assert preserved ownership and hierarchy and compare the same URLs and
  revisions before and after migration.
