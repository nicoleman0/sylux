//! Synthetic coordinator fixture shared by test-harness integration tests.
use crate::{
    domain::coordinator::MutationCoordinator,
    persistence::{
        schema,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use rusqlite::Connection;
use std::{
    fs::{self, OpenOptions},
    num::NonZeroUsize,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    time::Duration,
};

pub const PRINCIPAL: &str = "11111111111111111111111111111111";
pub const SESSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const SECRET: &[u8; 32] = b"0123456789abcdef0123456789abcdef";
pub const SCOPE: &str = "upload_create";
pub const NOW: i64 = 10;

pub struct Fixture {
    directory: PathBuf,
    pub workers: SqliteWorkers,
    pub coordinator: MutationCoordinator,
}

impl Fixture {
    pub fn create() -> Self {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).unwrap();
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let directory = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-revocation-{token}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let database = directory.join("metadata.sqlite3");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&database)
            .unwrap();
        let connection = sqlite::open_writer(&database, Duration::from_millis(500)).unwrap();
        schema::apply_migrations(&connection).unwrap();
        seed_account_and_session(&connection);
        drop(connection);
        let workers = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: Duration::from_millis(500),
                writer_queue_capacity: NonZeroUsize::new(8).unwrap(),
                reader_queue_capacity: NonZeroUsize::new(2).unwrap(),
            },
        )
        .unwrap();
        let coordinator = MutationCoordinator::new(workers.clone());
        Self {
            directory,
            workers,
            coordinator,
        }
    }

    pub async fn cleanup(self) {
        self.workers.shutdown().await.unwrap();
        for suffix in ["-wal", "-shm", ""] {
            let path = self.directory.join(format!("metadata.sqlite3{suffix}"));
            match fs::remove_file(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => panic!("synthetic fixture cleanup failed: {error}"),
            }
        }
        fs::remove_dir(self.directory).unwrap();
    }
}

pub fn seed_account_and_session(connection: &Connection) {
    connection.execute(
        "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'alice','user',1,0)",
        [PRINCIPAL],
    ).unwrap();
    connection.execute(
        "INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?3,?1,0,?2,?2,0,100,200,0)",
        rusqlite::params![PRINCIPAL, crate::domain::authz::verifier(SECRET).as_slice(), SESSION],
    ).unwrap();
}
