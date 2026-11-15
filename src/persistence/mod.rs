//! Local persistence foundation for root binding and SQLite workers.

pub mod instance;
pub mod ownership;
pub mod payload;
pub mod reconcile;
pub mod root;
pub mod schema;
pub mod sqlite;

/// Emit one redacted lifecycle event as JSON on stderr.
pub fn contract_event(name: &str) {
    eprintln!("{}", serde_json::json!({ "contract_event": name }));
}

pub fn contract_event_reason(name: &str, reason: &str) {
    eprintln!(
        "{}",
        serde_json::json!({ "contract_event": name, "reason": reason })
    );
}
