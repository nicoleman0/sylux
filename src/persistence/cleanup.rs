//! Resume only payload deletion authorized by a committed removal.
use super::{payload::PayloadStore, sqlite::SqliteWorkers};
use rusqlite::params;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, watch};

pub fn authorized(db: &rusqlite::Connection, blob: &str) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM removal_cleanup c JOIN removal r ON r.operation_id=c.operation_id JOIN blob b ON b.blob_id=c.blob_id WHERE c.blob_id=?1 AND b.lifecycle IN('deleting','deleted') AND NOT EXISTS(SELECT 1 FROM revision v WHERE v.blob_id=c.blob_id AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=v.entry_id)))",[blob],|r|r.get(0))
}

#[derive(Debug)]
pub enum CleanupError {
    Unavailable,
    Payload(super::payload::PayloadError),
}

// Keyset pages bound each runtime pass, including when an early candidate fails.
async fn batch(
    workers: &SqliteWorkers,
    store: &Arc<PayloadStore>,
    after: &mut String,
) -> Result<bool, CleanupError> {
    let cursor = after.clone();
    let candidates = workers.read(move |db| {
        let mut stmt = db.prepare("SELECT blob_id FROM removal_cleanup WHERE state='pending' AND blob_id>?1 ORDER BY blob_id LIMIT 32")?;
        let blobs = stmt.query_map([cursor], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for blob in &blobs {
            if !authorized(db, blob)? { return Err(rusqlite::Error::InvalidQuery); }
        }
        Ok(blobs)
    }).await.map_err(|_| CleanupError::Unavailable)?;
    let more = candidates.len() == 32;
    let mut failure = None;
    for blob in candidates {
        *after = blob.clone();
        let payloads = Arc::clone(store);
        let candidate = blob.clone();
        let unlinked = tokio::task::spawn_blocking(move || {
            payloads.unlink_authorized(&candidate)?;
            #[cfg(feature = "test-harness")]
            payloads.cleanup_boundary("cleanup.before_finalize")?;
            Ok::<_, super::payload::PayloadError>(())
        })
        .await
        .map_err(|_| CleanupError::Unavailable)?;
        if let Err(error) = unlinked {
            failure = Some(CleanupError::Payload(error));
            continue;
        }
        let finalized = workers
            .transact(move |tx| {
                if !authorized(tx, &blob)? {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                tx.execute(
                    "UPDATE blob SET lifecycle='deleted' WHERE blob_id=?1 AND lifecycle='deleting'",
                    [&blob],
                )?;
                tx.execute(
                    "UPDATE removal_cleanup SET state='complete' WHERE blob_id=?1",
                    params![blob],
                )?;
                Ok(())
            })
            .await;
        if finalized.is_err() {
            failure = Some(CleanupError::Unavailable);
        }
    }
    if !more {
        after.clear();
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(more),
    }
}

pub async fn resume(
    workers: &SqliteWorkers,
    store: &Arc<PayloadStore>,
) -> Result<(), CleanupError> {
    let mut after = String::new();
    while batch(workers, store, &mut after).await? {}
    Ok(())
}

/// The server owner joins this runner before closing SQLite or releasing the root.
pub struct Runner {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl Runner {
    pub fn start(workers: SqliteWorkers, store: Arc<PayloadStore>, wake: Arc<Notify>) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut after = String::new();
            let mut delay = Duration::from_secs(1);
            loop {
                tokio::select! {
                    _ = stopped.changed() => break,
                    _ = wake.notified() => {},
                    _ = tokio::time::sleep(delay) => {},
                }
                if *stopped.borrow() {
                    break;
                }
                // Do not cancel a pass: spawn_blocking must be joined before shutdown.
                match batch(&workers, &store, &mut after).await {
                    Ok(more) => {
                        delay = if more {
                            Duration::from_millis(10)
                        } else {
                            Duration::from_secs(1)
                        };
                    }
                    Err(_) => {
                        super::contract_event_reason(
                            "removal.cleanup_pending",
                            "cleanup_retry_required",
                        );
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                }
            }
        });
        Self { stop, task }
    }
    pub async fn shutdown(self) -> Result<(), CleanupError> {
        self.stop.send_replace(true);
        self.task.await.map_err(|_| CleanupError::Unavailable)
    }
}
