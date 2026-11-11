//! Local persistence foundation: root binding, exclusive ownership, the core
//! SQLite schema, verified per-connection policy and owned connection workers.
//!
//! Connection opening, migration and recovery checks are synchronous startup
//! operations. Serving submits short SQL closures to [`sqlite::SqliteWorkers`]:
//! one dedicated writer and one bounded read-only reader, each on its own
//! blocking worker. No network I/O may happen inside those SQL closures or
//! their transactions.

pub mod ownership;
pub mod payload;
pub mod reconcile;
pub mod root;
pub mod schema;
pub mod sqlite;

/// Emit a redacted lifecycle contract event on stderr as one JSON object per
/// line (s1-design-review §3/§5 startup trace). Events carry only allowlisted
/// names and machine reason codes — never paths, identities, credentials or
/// raw error text.
pub fn contract_event(name: &str) {
    eprintln!("{}", serde_json::json!({ "contract_event": name }));
}

/// [`contract_event`] with a redacted machine reason code.
pub fn contract_event_reason(name: &str, reason: &str) {
    eprintln!(
        "{}",
        serde_json::json!({ "contract_event": name, "reason": reason })
    );
}
