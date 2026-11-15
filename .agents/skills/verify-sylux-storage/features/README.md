# Storage feature map

Run the real HTTP and persistence checks from the repository root. All Rust targets below require `--features test-harness` and are registered in `Cargo.toml`.

| Feature | User paths | Map |
| --- | --- | --- |
| Uploads and downloads | Upload, commit, download, replace | [uploads](uploads.md) |
| Private files | Browse and access files as their owner; deny other accounts | [private files](private-files.md) |
| Trash recovery | Trash, inspect, restore, and reconcile after restart | [Trash recovery](trash-recovery.md) |

The tests construct synthetic data and owned `FixtureRoot` trees under the OS temporary directory. They bind loopback listeners on ephemeral ports and explicitly join their workers before successful cleanup. No user files or persistent app data are required.

For the initial executed run, see the status in [`../SKILL.md`](../SKILL.md). Real TLS browser checks and mocked UI smoke checks are separate evidence; see [`../SKILL.md`](../SKILL.md).
