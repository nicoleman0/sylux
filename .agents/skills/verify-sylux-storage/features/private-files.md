# Private files

## User paths and preconditions

The owner browses `GET /folders/children` and reads `GET` or `HEAD /entries/{id}/content`. Upload and mutation routes are listed in [`src/http/mod.rs`](../../../../src/http/mod.rs). The isolation fixture seeds owner, unrelated-user, and administrator identities with synthetic private entries.

## Drive

From the repository root, run:

```sh
cargo test --locked --features test-harness \
  --test contract_private_storage \
  --test contract_isolation \
  --test integration_personal_storage
```

Review `private_root_listing_returns_an_empty_page_with_generation` and `move_and_trash_require_if_match_without_changing_the_namespace` in [`tests/contract/private_storage.rs`](../../../../tests/contract/private_storage.rs), plus `a03_unrelated_and_admin_route_matrix_denies_owner_private_state` in [`tests/contract/isolation.rs`](../../../../tests/contract/isolation.rs). `personal_storage_routes_preserve_acknowledged_inventory_after_reconcile_restart` in [`tests/integration/personal_storage.rs`](../../../../tests/integration/personal_storage.rs) exercises setup, login, storage routes, logout, and restart against a fresh fixture.

## Observable proof and limits

Assertions distinguish an existing empty private root from a missing folder, require mutation preconditions, and deny unrelated users and administrators access to owner-private routes. Integration checks compare the inventory before and after restart and compare exact downloaded bytes.

These are real HTTP route and storage checks. Browser UI specs use mocked APIs and do not establish account isolation.
