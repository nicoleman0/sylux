## Context

The current storage model has an account-scoped namespace. Existing links
bind to a file and its current revision. Migration needs to preserve these
bindings as well as the existing hierarchy.

## Decisions

- Pending Nick's approval in SYL-110, use one private library per existing
  account and retain the hierarchy without asking owners to rearrange files.
  Update LIB-02 if the decision changes.
- Determine the storage representation and migration steps during SYL-110
  design. Check the result against LIB-02 using pre-upgrade data.

The pending migration decision belongs to SYL-110. Future sharing decisions
can be resolved in their own issues.
