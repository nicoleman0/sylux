//! Isolated memory qualification through the real TLS download route.
use super::*;
use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
};
use tokio::io::{AsyncBufReadExt, BufReader};

const MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const MEMBER_COUNT: usize = 65;
const TAIL_BYTES: usize = 64 * 1024;
const RSS_LIMIT: u64 = 256 * 1024 * 1024;
const CHILD_ENV: &str = "SYLUX_ARCHIVE_MEMORY_CHILD";
const TEST_NAME: &str = "large::multi_gib_tls_archive_has_bounded_memory_and_zip64_offsets";
const BLOB: &str = "ba000000000000000000000000000001";

#[test]
fn multi_gib_tls_archive_has_bounded_memory_and_zip64_offsets() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(download_large_archive());
        return;
    }

    // OS peak RSS includes the TLS client, server, SQLite workers and test runtime.
    let mut command = Command::new("/usr/bin/time");
    #[cfg(target_os = "macos")]
    command.arg("-l");
    #[cfg(not(target_os = "macos"))]
    command.arg("-v");
    let mut child = command
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("/usr/bin/time is required for archive peak-memory qualification");
    let started = Instant::now();
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() {
            break false;
        }
        if started.elapsed() > Duration::from_secs(120) {
            let group = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
            rustix::process::kill_process_group(group, rustix::process::Signal::KILL).unwrap();
            break true;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !timed_out,
        "archive download exceeded 120s\n{stdout}\n{stderr}"
    );
    assert!(
        output.status.success(),
        "archive child failed\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("large archive bytes="),
        "large child must run, not only discover tests"
    );
    let peak = peak_rss_bytes(&stderr);
    println!("{stdout}large archive peak_rss_bytes={peak}");
    assert!(
        peak < RSS_LIMIT,
        "archive peak RSS {peak} exceeded {RSS_LIMIT} bytes while streaming over 4 GiB"
    );
}

fn peak_rss_bytes(report: &str) -> u64 {
    #[cfg(target_os = "macos")]
    let value = report.lines().find_map(|line| {
        line.trim()
            .strip_suffix(" maximum resident set size")
            .map(|value| value.trim().parse::<u64>().unwrap())
    });
    #[cfg(not(target_os = "macos"))]
    let value = report.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Maximum resident set size (kbytes):")
            .map(|value| value.trim().parse::<u64>().unwrap() * 1024)
    });
    value
        .filter(|value| *value > 0)
        .expect("/usr/bin/time must report nonzero peak RSS")
}

fn seed_large(c: &Connection, product: &Path) {
    // One sparse immutable payload keeps disk use small while every member is verified and sent.
    let _store = PayloadStore::open(product).unwrap();
    let shard = product.join("payloads").join(&BLOB[..2]);
    fs::DirBuilder::new().mode(0o700).create(&shard).unwrap();
    let payload = common::create_private_file(&shard, BLOB).unwrap();
    payload.set_len(MEMBER_BYTES).unwrap();
    drop(payload);
    let buffer = [0u8; 64 * 1024];
    let mut sha = Sha256::new();
    for _ in 0..MEMBER_BYTES / buffer.len() as u64 {
        sha.update(buffer);
    }
    let digest: [u8; 32] = sha.finalize().into();
    let tx = c.unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,?2,?3,?4,'live')",
        params![BLOB, &BLOB[..2], MEMBER_BYTES as i64, digest.as_slice()],
    )
    .unwrap();
    for index in 0..MEMBER_COUNT {
        let id = format!("{value:032x}", value = 0x1000 + index);
        let revision = format!("{value:032x}", value = 0x2000 + index);
        let name = format!("part-{index:02}.bin");
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES(?1,?2,?3,'file',?4,?5,1,NULL,1)", params![id, OWNER, ROOT, name, name.as_bytes()]).unwrap();
        tx.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![revision, id, BLOB, MEMBER_BYTES as i64, digest.as_slice(), 1_709_214_358_i64, OWNER]).unwrap();
        tx.execute(
            "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
            params![revision, id],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

async fn download_large_archive() {
    let started = Instant::now();
    let f = Fixture::new("multi-gib", seed_large).await;
    let ids: Vec<_> = (0..MEMBER_COUNT)
        .map(|index| format!("{value:032x}", value = 0x1000 + index))
        .collect();
    let selected: Vec<_> = ids.iter().map(String::as_str).collect();
    let issued = f.issue(&selected, &OWNER_ACTOR).await;
    assert_eq!(issued.status, 201);
    assert_eq!(issued.json()["entry_count"], MEMBER_COUNT);
    let url = issued.json()["url"].as_str().unwrap().to_owned();
    let mut request = f.head("GET", &url, Some(&OWNER_ACTOR));
    request.push_str("\r\n");
    let mut stream = f.connect().await;
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reader = BufReader::with_capacity(64 * 1024, stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.starts_with("HTTP/1.1 200 "), "{line}");
    let mut chunked = false;
    loop {
        line.clear();
        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        chunked |= line.eq_ignore_ascii_case("transfer-encoding: chunked\r\n");
    }
    assert!(
        chunked,
        "archive must stream using chunked transfer encoding"
    );
    let mut tail = vec![0u8; TAIL_BYTES];
    let mut cursor = 0;
    let mut total = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).await.unwrap(),
            0,
            "missing terminating HTTP chunk"
        );
        let mut remaining = usize::from_str_radix(line.trim(), 16).unwrap();
        if remaining == 0 {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert_eq!(line, "\r\n");
            break;
        }
        while remaining > 0 {
            let take = remaining.min(buffer.len());
            reader.read_exact(&mut buffer[..take]).await.unwrap();
            let first = take.min(TAIL_BYTES - cursor);
            tail[cursor..cursor + first].copy_from_slice(&buffer[..first]);
            tail[..take - first].copy_from_slice(&buffer[first..take]);
            cursor = (cursor + take) % TAIL_BYTES;
            total += take as u64;
            remaining -= take;
        }
        let mut crlf = [0; 2];
        reader.read_exact(&mut crlf).await.unwrap();
        assert_eq!(&crlf, b"\r\n");
    }
    tail.rotate_left(cursor);
    verify_large_directory(&tail, total);
    drop(reader);
    f.close().await;
    println!(
        "large archive bytes={total} elapsed_ms={}",
        started.elapsed().as_millis()
    );
}

fn verify_large_directory(tail: &[u8], total: u64) {
    assert!(total > u64::from(u32::MAX), "must exercise ZIP64 offsets");
    let end = tail.len() - 22;
    let locator = end - 20;
    let zip64 = locator - 56;
    assert_eq!(u32_at(tail, end), 0x0605_4b50);
    assert_eq!(u32_at(tail, end + 16), u32::MAX);
    assert_eq!(u32_at(tail, locator), 0x0706_4b50);
    assert_eq!(u64_at(tail, locator + 8), total - 98);
    assert_eq!(u32_at(tail, zip64), 0x0606_4b50);
    assert_eq!(u64_at(tail, zip64 + 32), MEMBER_COUNT as u64);
    let size = u64_at(tail, zip64 + 40);
    let offset = u64_at(tail, zip64 + 48);
    assert_eq!(offset + size, total - 98);
    let mut at = usize::try_from(offset - (total - tail.len() as u64)).unwrap();
    let mut local = 0u64;
    let mut expected_crc = crc32fast::Hasher::new();
    let zeros = [0; 64 * 1024];
    for _ in 0..MEMBER_BYTES / zeros.len() as u64 {
        expected_crc.update(&zeros);
    }
    let expected_crc = expected_crc.finalize();
    for index in 0..MEMBER_COUNT {
        assert_eq!(u32_at(tail, at), 0x0201_4b50);
        assert_eq!(u32_at(tail, at + 16), expected_crc);
        assert_eq!(u32_at(tail, at + 20), MEMBER_BYTES as u32);
        assert_eq!(u32_at(tail, at + 24), MEMBER_BYTES as u32);
        let name_length = usize::from(u16_at(tail, at + 28));
        let extra_length = usize::from(u16_at(tail, at + 30));
        let name = format!("part-{index:02}.bin");
        assert_eq!(&tail[at + 46..at + 46 + name_length], name.as_bytes());
        if local >= u64::from(u32::MAX) {
            assert_eq!(u32_at(tail, at + 42), u32::MAX);
            let extra = at + 46 + name_length;
            assert_eq!(u16_at(tail, extra), 1, "ZIP64 extra field");
            assert_eq!(u16_at(tail, extra + 2), 8);
            assert_eq!(u64_at(tail, extra + 4), local);
        } else {
            assert_eq!(u64::from(u32_at(tail, at + 42)), local);
        }
        local += 30 + name.len() as u64 + MEMBER_BYTES + 16;
        at += 46 + name_length + extra_length;
    }
    assert_eq!(local, offset, "all member bytes precede the directory");
    assert_eq!(
        at, zip64,
        "central directory must contain exactly the selected members"
    );
}
