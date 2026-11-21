# Private storage

## Purpose

Keep an account's private files and folders accessible through its own
signed-in session. The separate [single-file link workflow](../../../docs/sharing.md)
covers public downloads an owner explicitly authorizes.

## Requirements

### Requirement: Account isolation for private files and folders

Signed-in folder listings and file content requests SHALL be scoped to the
owning account. Administrator status SHALL confer no access to another
account's private folders or file contents.

#### Scenario: Another account requests a private listing or file

- **GIVEN** an owner can list a private folder and read a private file
- **WHEN** an unrelated account or an administrator requests the same listing
  or file content using its own session
- **THEN** the server denies access without returning private names or content

The existing HTTP check is
[`a03_unrelated_and_admin_route_matrix_denies_owner_private_state`](../../../tests/contract/isolation.rs).
Run it after the [normal build preparation](../../../AGENTS.md#build-and-verification).

```sh
cargo test --locked --features test-harness --test contract_isolation a03_unrelated_and_admin_route_matrix_denies_owner_private_state
```
