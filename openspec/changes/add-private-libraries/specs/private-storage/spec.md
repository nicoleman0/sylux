## ADDED Requirements

### Requirement: Named private libraries

An owner SHALL be able to create and browse named libraries in My libraries.
Each library SHALL have a distinct identity. New libraries SHALL be private
to their owner.

#### Scenario: LIB-01 Separate libraries with matching folder names

- **GIVEN** an owner creates two libraries with identically named child folders
  containing different files
- **WHEN** the owner opens each library with the keyboard, reloads, and
  reopens its saved URL
- **THEN** each library shows its own contents
- **AND** a second account finds neither library in navigation or search and
  cannot open either through a direct request

### Requirement: Private library migration preserves existing access

Upgrade SHALL preserve owners' access to their existing hierarchy, ownership,
and the identity, revision, expiry and revocation state of existing single-file
links. Migration SHALL create no sharing grants or folder links.

#### Scenario: LIB-02 Existing files and links survive upgrade

- **GIVEN** existing accounts with folders, files and active, expired and
  revoked single-file links
- **WHEN** the instance upgrades
- **THEN** owners browse their original hierarchy in one private library per
  account and ownership is unchanged
- **AND** active URLs still serve the same file revisions until expiry or revocation
- **AND** expired and revoked URLs still fail
- **AND** migration gives no new access to another account or a signed-out visitor
