//! Opt-in metadata fixture. Seeded file payloads are absent and must never be read.
#[allow(unused_imports)]
#[path = "common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, Write},
    os::unix::fs::DirBuilderExt,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sylux::{
    domain::{
        authz::{PasswordLimiter, verifier},
        names::EntryName,
        namespace::{self, ListingSort},
        search,
    },
    http::{self, AuthState},
    persistence::{
        root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tower_http::services::ServeDir;

const OWNER: &str = "00000000000000000000000000000001";
const ROOT: &str = "00000000000000000000000000000002";
const SEED: u64 = 48;
const BUSY: Duration = Duration::from_millis(375);
fn id(n: usize) -> String {
    format!("{n:032x}")
}
fn fail(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{e:?}"))
}
fn emit(v: Value) -> io::Result<()> {
    println!("{v}");
    io::stdout().flush()
}

type Expected = (String, String, bool, i64, i64);
fn ordered_ids(rows: &[Expected]) -> Value {
    let mut result = serde_json::Map::new();
    for sort in [
        "name_asc",
        "name_desc",
        "size_asc",
        "size_desc",
        "modified_asc",
        "modified_desc",
    ] {
        let mut ordered = rows.to_vec();
        ordered.sort_by(|a, b| {
            a.2.cmp(&b.2)
                .then_with(|| {
                    if !a.2 {
                        return a.1.cmp(&b.1);
                    }
                    match sort {
                        "name_desc" => b.1.cmp(&a.1),
                        "size_asc" => a.3.cmp(&b.3),
                        "size_desc" => b.3.cmp(&a.3),
                        "modified_asc" => a.4.cmp(&b.4),
                        "modified_desc" => b.4.cmp(&a.4),
                        _ => a.1.cmp(&b.1),
                    }
                    .then_with(|| a.1.cmp(&b.1))
                })
                .then_with(|| a.0.cmp(&b.0))
        });
        result.insert(
            sort.into(),
            json!(
                ordered
                    .iter()
                    .take(1000)
                    .map(|row| &row.0)
                    .collect::<Vec<_>>()
            ),
        );
    }
    Value::Object(result)
}

fn seed(db: &mut Connection, count: usize, password_hash: &str) -> io::Result<Value> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(fail)?
        .as_secs() as i64;
    let tx = db.transaction().map_err(fail)?;
    tx.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,'measure-owner','user',1,0,?2)", params![OWNER,password_hash]).map_err(fail)?;
    tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)", params![ROOT,OWNER]).map_err(fail)?;
    let mut sessions = Vec::new();
    let mut expected_root = Vec::new();
    let mut expected_deep = Vec::new();
    let mut expected_live_files = Vec::new();
    let mut expected_trash = Vec::new();
    for n in 0..10 {
        let mut secret = [0u8; 32];
        let mut csrf = [0u8; 32];
        getrandom::fill(&mut secret).map_err(fail)?;
        getrandom::fill(&mut csrf).map_err(fail)?;
        let sid = id(0x100 + n);
        tx.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)", params![sid,OWNER,verifier(&secret).as_slice(),verifier(&csrf).as_slice(),now,now+7200,now+14400]).map_err(fail)?;
        sessions.push(json!({"cookie":format!("{sid}.{}",URL_SAFE_NO_PAD.encode(secret)),"csrf":URL_SAFE_NO_PAD.encode(csrf),"folder":id(0x200+n)}));
    }
    {
        let mut entry = tx.prepare("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation,state) VALUES(?1,?2,?3,?4,?5,?6,1,1,?7)").map_err(fail)?;
        let mut blob = tx.prepare("INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,substr(?1,1,2),?2,zeroblob(32),'live')").map_err(fail)?;
        let mut rev = tx.prepare("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,zeroblob(32),?5,?6)").map_err(fail)?;
        let mut current = tx
            .prepare("UPDATE entry SET current_revision_id=?2 WHERE entry_id=?1")
            .map_err(fail)?;
        for n in 0..10 {
            let name = EntryName::parse(&format!("uploads-{n:02}")).map_err(fail)?;
            expected_root.push((id(0x200 + n), name.display.clone(), false, 0, 0));
            entry
                .execute(params![
                    id(0x200 + n),
                    OWNER,
                    ROOT,
                    "folder",
                    name.display,
                    name.key,
                    "active"
                ])
                .map_err(fail)?;
        }
        for n in 0..count {
            let eid = id(0x1000 + n);
            let parent = if n < 16 {
                if n == 0 {
                    ROOT.into()
                } else {
                    id(0x1000 + n - 1)
                }
            } else if n < count * 4 / 5 {
                ROOT.into()
            } else {
                id(0x1000 + (n % 16))
            };
            let name = EntryName::parse(&if n < 16 {
                format!("deep-{n:02}")
            } else {
                format!("item-{n:06}.txt")
            })
            .map_err(fail)?;
            let kind = if n < 16 { "folder" } else { "file" };
            let state = if n >= 16 && n % 10 == 0 {
                "deleted"
            } else {
                "active"
            };
            let length = if n >= 16 {
                ((n as u64 * 7919 + SEED) % 16_777_216) as i64
            } else {
                0
            };
            let modified = 1_700_000_000 + (n % 86400) as i64;
            if state == "active" {
                let row = (
                    eid.clone(),
                    name.display.clone(),
                    kind == "file",
                    length,
                    modified,
                );
                if parent == ROOT {
                    expected_root.push(row.clone());
                }
                if parent == id(0x1000 + 15) {
                    expected_deep.push(row);
                }
                if kind == "file" {
                    expected_live_files.push(eid.clone());
                }
            } else {
                expected_trash.push(eid.clone());
            }
            entry
                .execute(params![
                    eid,
                    OWNER,
                    parent,
                    kind,
                    name.display,
                    name.key,
                    state
                ])
                .map_err(fail)?;
            if n >= 16 {
                let bid = id(0x1000000 + n);
                let rid = id(0x2000000 + n);
                blob.execute(params![bid, length]).map_err(fail)?;
                rev.execute(params![rid, eid, bid, length, modified, OWNER])
                    .map_err(fail)?;
                current.execute(params![eid, rid]).map_err(fail)?;
            }
        }
    }
    tx.commit().map_err(fail)?;
    let deleted: i64 = db
        .query_row(
            "SELECT count(*) FROM entry WHERE state='deleted'",
            [],
            |r| r.get(0),
        )
        .map_err(fail)?;
    let wide_live: i64 = db
        .query_row(
            "SELECT count(*) FROM entry WHERE parent_id=?1 AND state='active'",
            [ROOT],
            |r| r.get(0),
        )
        .map_err(fail)?;
    let mut layers = Vec::new();
    for n in 0..16 {
        let live: i64 = db
            .query_row(
                "SELECT count(*) FROM entry WHERE parent_id=?1 AND state='active'",
                [id(0x1000 + n)],
                |r| r.get(0),
            )
            .map_err(fail)?;
        layers.push(live);
    }
    Ok(
        json!({"sessions":sessions,"expectations":{"root":ordered_ids(&expected_root),"deep":ordered_ids(&expected_deep),"path":std::iter::once(ROOT.to_owned()).chain((0..16).map(|n|id(0x1000+n))).collect::<Vec<_>>(),"search_ids":{"selective":vec![id(0x1000+99)],"broad":expected_live_files.iter().take(100).collect::<Vec<_>>(),"none":Vec::<String>::new(),"one":expected_live_files.iter().take(100).collect::<Vec<_>>()},"search_counts":{"selective":1,"broad":expected_live_files.len(),"none":0,"one":expected_live_files.len()},"trash":expected_trash},"dimensions":{"seed":SEED,"metadata_entries":count,"wide_root_live_children":wide_live,"deep_live_children_by_layer":layers,"total_seed_entries":count+11,"metadata_files":count-16,"deep_folder_depth":16,"trash_entries":deleted,"sessions":10,"payloads":"metadata-only; absent; never request content","size_formula":"(index * 7919 + seed) % 16777216","modified_base_unix":1700000000}}),
    )
}

fn search_plan(db: &Connection) -> io::Result<Vec<String>> {
    let source = include_str!("../src/domain/search.rs");
    let literal = source
        .split("const LIVE_SQL: &str =")
        .nth(1)
        .and_then(|part| part.split("\";").next())
        .ok_or_else(|| fail("search SQL source changed"))?;
    let sql = literal
        .trim()
        .trim_matches('"')
        .lines()
        .map(|line| line.trim().trim_end_matches('\\'))
        .collect::<Vec<_>>()
        .join(" ");
    let depth = include_str!("../src/domain/namespace.rs")
        .lines()
        .find(|line| line.starts_with("pub(super) const MAX_ENTRY_DEPTH:"))
        .and_then(|line| line.split('=').nth(1))
        .ok_or_else(|| fail("depth constant source changed"))?
        .trim()
        .trim_end_matches(';')
        .parse::<i64>()
        .map_err(fail)?;
    let mut statement = db
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .map_err(fail)?;
    statement
        .query_map(params![ROOT, OWNER, depth], |row| row.get(3))
        .map_err(fail)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(fail)
}

async fn probes(workers: &SqliteWorkers, fast: bool) -> io::Result<Vec<Value>> {
    let mut tasks = Vec::new();
    for n in 0..12 {
        let workers = workers.clone();
        tasks.push(tokio::spawn(async move {
            let operation=if fast { ["list_name","list_size","list_modified","trash"][n%4] } else { ["list_name","list_size","list_modified","search_selective","search_broad","search_none","search_one","trash"][n%8] };
            let submitted=Instant::now();
            workers.read(move |db| {
                let started=Instant::now();
                let result=match operation {
                    "list_name"=>namespace::list_sorted(db,OWNER,ROOT,ListingSort::NameAsc,None,100).map(|v|v.len()),
                    "list_size"=>namespace::list_sorted(db,OWNER,ROOT,ListingSort::SizeDesc,None,100).map(|v|v.len()),
                    "list_modified"=>namespace::list_sorted(db,OWNER,ROOT,ListingSort::ModifiedDesc,None,100).map(|v|v.len()),
                    "trash"=>namespace::trashed(db,OWNER).map(|v|v.len()),
                    _=>search::find(db,OWNER,match operation {"search_selective"=>"item-000099", "search_broad"=>"item", "search_none"=>"zzzz-no-match",_=>"i"}).map(|v|v.hits.len())
                }.map_err(|_|rusqlite::Error::InvalidQuery)?;
                Ok(json!({"operation":operation,"concurrent_probe_batch":12,"queue_wait_ms":started.duration_since(submitted).as_secs_f64()*1000.,"service_ms":started.elapsed().as_secs_f64()*1000.,"rows":result}))
            }).await.map_err(fail)
        }));
    }
    let mut out = Vec::new();
    for task in tasks {
        out.push(task.await.map_err(fail)??);
    }
    Ok(out)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> io::Result<()> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() != Some("--run") {
        println!("Opt-in benchmark skipped; use scripts/measure-workflows.mjs");
        return Ok(());
    }
    let argument = arguments
        .next()
        .ok_or_else(|| fail("--run requires an entry count"))?;
    if arguments.next().is_some() {
        return Err(fail("unexpected benchmark argument"));
    }
    let count = argument.parse::<usize>().map_err(fail)?;
    if !(100..=1_000_000).contains(&count) {
        return Err(fail("count outside 100..1000000"));
    }
    let parent = fs::canonicalize(std::env::temp_dir())?
        .join(format!("sylux-measure-{}", common::unique_token()));
    let fixture = FixtureRoot::create(&parent, "measure", "workflows")?;
    let product = fixture.path().join("product");
    fs::DirBuilder::new().mode(0o700).create(&product)?;
    root::initialize_empty_root(&product, BUSY).map_err(fail)?;
    let tls = TlsFiles::generate_browser(&fixture, "tls")?;
    let db_path = product.join("metadata/sylux.sqlite3");
    let mut db = sqlite::open_writer(&db_path, BUSY).map_err(fail)?;
    let password = common::unique_token();
    let password_hash = PasswordLimiter::default()
        .hash_password(password.clone().into_bytes())
        .await
        .ok_or_else(|| fail("hash failed"))?;
    let mut seeded = seed(&mut db, count, &password_hash)?;
    seeded["dimensions"]["search_query_plan"] = json!(search_plan(&db)?);
    let workers = SqliteWorkers::from_writer(
        db,
        &db_path,
        WorkerConfig {
            busy_timeout: BUSY,
            ..WorkerConfig::default()
        },
    )
    .map_err(fail)?;
    let lease = fixture.register_worker()?;
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/dist");
    let mut server =
        TlsApiServer::start_with_origin(&fixture, &tls, Duration::from_secs(3), |origin| {
            http::router(
                AuthState::new(workers.clone(), origin)
                    .with_storage_root(&product)
                    .expect("fixture storage"),
            )
            .fallback_service(ServeDir::new(assets).append_index_html_on_directories(true))
        })
        .await?;
    emit(
        json!({"event":"ready","pid":std::process::id(),"origin":format!("https://fixture.test:{}",server.address().port()),"ca":tls.ca_cert,"leaf":tls.leaf_cert,"password":password,"sessions":seeded["sessions"],"dimensions":seeded["dimensions"],"root":fixture.path(),"expectations":seeded["expectations"],"embedded_fixture_sha256":common::sha256_hex(include_bytes!("performance_fixture.rs")),"embedded_assets_index_sha256":common::sha256_hex(include_bytes!("../web/dist/index.html")),"source_at_build":option_env!("SYL48_SOURCE_SHA"),"schema_version":sylux::persistence::schema::SUPPORTED_SCHEMA_VERSION}),
    )?;
    // A dedicated input thread exits at EOF; stop closes stdin before cleanup.
    let (sender, mut commands) = tokio::sync::mpsc::channel(4);
    let input = std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let line = line?;
            let stopping = line == "stop";
            if sender.blocking_send(line).is_err() || stopping {
                break;
            }
        }
        Ok::<_, io::Error>(())
    });
    let outcome = async {
        loop {
            let command = tokio::time::timeout(Duration::from_secs(1800), commands.recv())
                .await
                .map_err(fail)?;
            match command.as_deref() {
                Some("probes") | Some("probes-fast") => {
                    emit(json!({"event":"probes","samples":probes(&workers, command.as_deref() == Some("probes-fast")).await?}))?
                }
                Some("stop") | None => break,
                _ => return Err(fail("unknown fixture command")),
            }
        }
        Ok::<_, io::Error>(())
    }
    .await;
    server.shutdown_and_join().await?;
    workers.shutdown().await.map_err(fail)?;
    lease.joined()?;
    if input.is_finished() {
        input.join().map_err(fail)??;
    } else {
        return Err(fail("input thread still active; retain fixture"));
    }
    fixture.cleanup_after_workers_exit()?;
    fs::remove_dir(parent)?;
    emit(json!({"event":"stopped","joined":true,"cleaned":true}))?;
    outcome
}
