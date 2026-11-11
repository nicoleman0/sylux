# Native LXC installation scope

Docker is the primary beta installation. Native LXC is a secondary path for
operators who manage the guest OS, service account, TLS and systemd themselves.
There is no separately published native-binary artifact or qualified Proxmox
template in this beta. Running Docker inside LXC is not the reference deployment.

A native installation must use the same source revision as the selected release,
build the embedded UI before the Rust binary, and run one service identity against
one private local data root. Keep TLS keys outside that root. The guest needs a
compatible Linux amd64 userspace and the runtime libraries used by the build.
Do not assume a binary built on another distribution is portable.

LXC storage and isolation require their own checks: UID mapping and ownership,
file and directory synchronization, writer exclusion, service shutdown, backups
and restore. Keep the service private, verify TLS at both proxy hops, and set
`--public-origin` to the exact browser origin. Do not copy a proxy configuration
that disables backend certificate verification.

The Docker CI result does not qualify LXC storage, permissions or restore. Use the
[Docker guide](docker-beta.md) for the supported beta onboarding path until the
specific LXC environment has its own recorded evidence. Send environment details
and failures to hello@corvus-dev.com; do not include credentials or personal files.
