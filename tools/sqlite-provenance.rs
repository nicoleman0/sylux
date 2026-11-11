//! Standalone build-lock diagnostic, not the storage adapter or service.
fn inspect(connection: &rusqlite::Connection) -> rusqlite::Result<serde_json::Value> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; \
         PRAGMA busy_timeout=5000; PRAGMA wal_autocheckpoint=1000;",
    )?;
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let busy_timeout: i64 = connection.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
    let autocheckpoint: i64 =
        connection.query_row("PRAGMA wal_autocheckpoint", [], |r| r.get(0))?;
    let (version, source_id): (String, String) =
        connection.query_row("SELECT sqlite_version(), sqlite_source_id()", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let mut statement = connection.prepare("PRAGMA compile_options")?;
    let mut options = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    options.sort();
    Ok(serde_json::json!({
        "sqlite_version": version,
        "sqlite_source_id": source_id,
        "compile_options": options,
        "journal_mode": journal_mode,
        "synchronous": synchronous,
        "foreign_keys": foreign_keys,
        "busy_timeout_ms": busy_timeout,
        "wal_autocheckpoint_pages": autocheckpoint,
        "build_contract": {
            "target": "x86_64-unknown-linux-gnu",
            "rusqlite": "0.41.1",
            "libsqlite3_sys": "0.39.1",
            "selected_feature": "bundled",
            "expected_linking": "static bundled SQLite; independently verify native build and ELF",
        },
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut arguments = std::env::args_os().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: sqlite-provenance <NEW-disposable-database>")?;
    if arguments.next().is_some() {
        return Err("expected exactly one new disposable database path".into());
    }
    // Refuse existing state. This tool is not safe to run on an application DB.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    drop(file);
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let report = inspect(&connection)?;
    if report["sqlite_version"] != "3.54.1"
        || report["sqlite_source_id"]
            != "2026-10-16 14:45:20 555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76"
        || report["journal_mode"] != "wal"
        || report["synchronous"] != 2
        || report["foreign_keys"] != 1
        || report["busy_timeout_ms"] != 5000
        || report["wal_autocheckpoint_pages"] != 1000
    {
        return Err(
            "unexpected linked SQLite identity or configuration; qualification blocked".into(),
        );
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_wal_full_and_foreign_keys_on_a_real_disposable_database() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".local/probe-tests");
        std::fs::create_dir_all(&directory).unwrap();
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).unwrap();
        let name = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let connection = rusqlite::Connection::open(directory.join(name)).unwrap();
        let report = inspect(&connection).unwrap();
        assert_eq!(report["journal_mode"], "wal");
        assert_eq!(report["synchronous"], 2);
        assert_eq!(report["foreign_keys"], 1);
        assert_eq!(report["busy_timeout_ms"], 5000);
        assert_eq!(report["wal_autocheckpoint_pages"], 1000);
    }

    #[test]
    fn reports_the_engine_actually_linked_into_this_process() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let report = inspect(&connection).unwrap();
        assert_eq!(report["sqlite_version"], "3.54.1");
        assert_eq!(
            report["sqlite_source_id"],
            "2026-10-16 14:45:20 555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76"
        );
        assert!(
            report["compile_options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "THREADSAFE=1")
        );
    }
}
