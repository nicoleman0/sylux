# Huskarl notice provenance

The crates.io archives for `huskarl 0.11.11`, `huskarl-core 0.10.11` and
`huskarl-macros 0.4.2` omit both workspace license files. Each archive's
`.cargo_vcs_info.json` points to its matching subdirectory at the same
[upstream revision](https://github.com/huskarl-rs/huskarl/tree/b2026452685646d13b3a987db36d17a0ec5eda75).
The three manifests declare `MIT OR Apache-2.0`. The files here are verbatim
copies of that revision's root
[LICENSE-APACHE](https://github.com/huskarl-rs/huskarl/blob/b2026452685646d13b3a987db36d17a0ec5eda75/LICENSE-APACHE)
and [LICENSE-MIT](https://github.com/huskarl-rs/huskarl/blob/b2026452685646d13b3a987db36d17a0ec5eda75/LICENSE-MIT).

The collector checks the package identity, the cached archive's SHA-256
against the reviewed checksums from `Cargo.lock`, and both license digests.
If a future release still omits notices, verify its archive and upstream files
before changing these pins. When the resolved releases ship their own notices,
remove the corresponding pins and delete these files once no pins remain.
