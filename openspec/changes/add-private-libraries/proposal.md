# Add private libraries

## Why

[SYL-110](https://linear.app/corvus-dev/issue/SYL-110/create-named-libraries-and-browse-existing-files-from-my-libraries)
gives owners named libraries and a place to find the files they already have.

## What Changes

- Add owner-only library creation and browsing through My libraries.
- Bring the existing hierarchy into My libraries while preserving ownership
  and existing single-file links.
- Keep account grants, instance publication and anonymous folder links outside
  this change.

Read the [proposed rules](specs/private-storage/spec.md) and
[planned checks](tasks.md) for LIB-01 and LIB-02.

## Capabilities

### Modified Capabilities

- `private-storage`: Add named libraries and migration to the existing
  private storage behavior.

## Impact

This proposal affects the stored namespace, upgrade path and Files navigation.
Use the shipped single-file link behavior in `docs/sharing.md` as the
compatibility reference.
