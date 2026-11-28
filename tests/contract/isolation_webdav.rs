use super::{ADMIN_CSRF, CSRF, Fixture, OTHER, OWNER, STANDARD, cookie};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::time::Duration;

#[tokio::test]
async fn webdav_option_credentials_read_only_and_owner_isolation() {
    let off = Fixture::new().await;
    assert_eq!(off.request("OPTIONS", "/dav/", None, &[], "").await.0, 404);
    assert_eq!(
        off.request("GET", "/me/webdav", cookie(0), &[], "").await.0,
        404
    );
    off.close().await;

    let fixture = Fixture::with_webdav(true).await;
    let path = "/dav/Files";
    let (status, headers, _) = fixture
        .request("OPTIONS", "/dav/", cookie(0), &[], "")
        .await;
    assert_eq!(status, 401);
    assert!(headers["www-authenticate"].starts_with("Basic "));
    let browser_password = (
        "Authorization",
        format!("Basic {}", STANDARD.encode("owner:ordinary password")),
    );
    assert_eq!(
        fixture
            .request("OPTIONS", "/dav/", None, &[browser_password], "")
            .await
            .0,
        401
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                "/me/webdav",
                cookie(0),
                &[],
                r#"{"name":"stray","read_only":false}"#
            )
            .await
            .0,
        403
    );

    let (owner_id, owner) = fixture.credential(0, "Reader", true).await;
    let (_, other) = fixture.credential(1, "Other device", false).await;
    let (_, admin) = fixture.credential(2, "Administrator device", false).await;
    let options = fixture
        .request("OPTIONS", path, None, std::slice::from_ref(&owner), "")
        .await;
    assert_eq!(options.0, 204);
    assert_eq!(options.1["allow"], "OPTIONS, PROPFIND, GET, HEAD");
    let depth = ("Depth", "1".into());
    for credential in [&owner, &other, &admin] {
        let (status, _, body) = fixture
            .request(
                "PROPFIND",
                path,
                None,
                &[credential.clone(), depth.clone()],
                "",
            )
            .await;
        assert_eq!(status, 207);
        let xml = std::str::from_utf8(&body).unwrap();
        assert_eq!(xml.contains("private-file.bin"), credential == &owner);
        assert_eq!(xml.contains("private-name"), credential == &owner);
    }
    for credential in [&other, &admin] {
        assert_eq!(
            fixture
                .request(
                    "GET",
                    "/dav/Files/private-file.bin",
                    None,
                    std::slice::from_ref(credential),
                    ""
                )
                .await
                .0,
            404
        );
    }
    let source = "/dav/Files/private-file.bin";
    for (method, target) in [
        ("PUT", source),
        ("MKCOL", "/dav/Files/no"),
        ("MOVE", source),
        ("DELETE", source),
    ] {
        let headers = [owner.clone(), ("Destination", "/dav/Files/no".into())];
        assert_eq!(
            fixture.request(method, target, None, &headers, "").await.0,
            403,
            "{method}"
        );
    }
    assert_eq!(
        fixture
            .request("GET", source, None, std::slice::from_ref(&owner), "")
            .await
            .2,
        b"synthetic isolation payload"
    );
    let (_, _, listed) = fixture
        .request("GET", "/me/webdav", cookie(0), &[], "")
        .await;
    let listed: serde_json::Value = serde_json::from_slice(&listed).unwrap();
    assert_eq!(
        listed,
        serde_json::json!([{ "id": owner_id, "name": "Reader", "read_only": true }])
    );

    let revoke_path = format!("/me/webdav/{owner_id}");
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &revoke_path,
                cookie(2),
                &fixture.mutation_headers(ADMIN_CSRF),
                ""
            )
            .await
            .0,
        404
    );
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &revoke_path,
                cookie(0),
                &fixture.mutation_headers(CSRF),
                ""
            )
            .await
            .0,
        204
    );
    assert_eq!(
        fixture.request("GET", source, None, &[owner], "").await.0,
        401
    );

    let admin_headers = fixture.mutation_headers(ADMIN_CSRF);
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/accounts/{OTHER}/disable"),
                cookie(2),
                &admin_headers,
                ""
            )
            .await
            .0,
        204
    );
    assert_eq!(
        fixture
            .request("OPTIONS", "/dav/", None, std::slice::from_ref(&other), "")
            .await
            .0,
        401
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/accounts/{OTHER}/enable"),
                cookie(2),
                &admin_headers,
                ""
            )
            .await
            .0,
        204
    );
    assert_eq!(
        fixture
            .request("OPTIONS", "/dav/", None, &[other], "")
            .await
            .0,
        401
    );
    fixture.close().await;
}

#[tokio::test]
async fn webdav_storage_writes_preserve_bytes_preconditions_and_trash() {
    let fixture = Fixture::with_webdav(true).await;
    let (_, auth) = fixture.credential(0, "Desktop", false).await;
    let credentials = [auth.clone()];
    let origin = fixture.dav_origin();
    let auth_headers = fixture.mutation_headers(CSRF);
    let (status, _, _) = fixture
        .request(
            "POST",
            "/libraries",
            cookie(0),
            &auth_headers,
            r#"{"name":"Work & notes"}"#,
        )
        .await;
    assert_eq!(status, 201);
    let (status, _, root_xml) = fixture
        .request(
            "PROPFIND",
            "/dav/",
            None,
            &[auth.clone(), ("Depth", "1".into())],
            "",
        )
        .await;
    assert_eq!(status, 207);
    let root_xml = std::str::from_utf8(&root_xml).unwrap();
    assert!(root_xml.contains("/dav/Work%20%26%20notes/"));
    assert!(root_xml.contains("Work &amp; notes"));
    for (actor, name) in [(1, "Other libraries"), (2, "Admin libraries")] {
        let (_, credential) = fixture.credential(actor, name, false).await;
        assert_eq!(
            fixture
                .request(
                    "PROPFIND",
                    "/dav/Work%20%26%20notes/",
                    None,
                    &[credential, ("Depth", "0".into())],
                    ""
                )
                .await
                .0,
            404
        );
    }
    assert_eq!(
        fixture
            .request("DELETE", "/dav/Files/", None, &credentials, "")
            .await
            .0,
        403
    );

    for path in [
        "/dav/Files/..",
        "/dav/Files/%2e%2e",
        "/dav/Files/%2F",
        "/dav/Files/%5c",
        "/dav/Files/%FF",
        "/dav/Files/%",
        "/dav/Files//bad",
        "/dav/Files?discard=yes",
    ] {
        assert_eq!(
            fixture.request("GET", path, None, &credentials, "").await.0,
            400,
            "{path}"
        );
    }
    let prop = r#"<x:propfind xmlns:x="DAV:" xmlns:t="urn:test"><x:prop><x:resourcetype/><t:unknown/></x:prop></x:propfind>"#;
    let (status, _, xml) = fixture
        .request(
            "PROPFIND",
            "/dav/Files",
            None,
            &[auth.clone(), ("Depth", "0".into())],
            prop,
        )
        .await;
    assert_eq!(status, 207);
    let xml = std::str::from_utf8(&xml).unwrap();
    assert!(xml.contains("404 Not Found") && xml.contains("urn:test"));
    assert_eq!(
        fixture
            .request(
                "PROPFIND",
                "/dav/Files",
                None,
                &[auth.clone(), ("Depth", "1".into())],
                "<bogus>"
            )
            .await
            .0,
        400
    );
    assert_eq!(
        fixture
            .request("PROPFIND", "/dav/Files", None, &credentials, "")
            .await
            .0,
        403
    );
    for method in ["LOCK", "UNLOCK", "COPY", "PROPPATCH", "PATCH", "POST"] {
        let (status, headers, _) = fixture
            .request(method, "/dav/Files", None, &credentials, "")
            .await;
        assert_eq!(status, 405, "{method}");
        assert!(!headers["allow"].split(", ").any(|item| item == method));
    }

    let folder = "/dav/Work%20%26%20notes/Delivery";
    assert_eq!(
        fixture
            .request("MKCOL", folder, None, &credentials, "")
            .await
            .0,
        201
    );
    assert_eq!(
        fixture
            .request("MKCOL", folder, None, &credentials, "")
            .await
            .0,
        405
    );
    let bytes = [0, 255, 42, 10, 0, 2, 245];
    let source = "/dav/Files/caf%C3%A9%20%25draft.bin";
    let (status, headers, _) = fixture
        .request_bytes(
            "PUT",
            source,
            None,
            &[auth.clone(), ("If-None-Match", "*".into())],
            &bytes,
        )
        .await;
    assert_eq!(status, 201);
    let tag = headers["etag"].clone();
    let canon = "/dav/Files/cafe%CC%81%20%25draft.bin";
    let (status, _, original) = fixture.request("GET", canon, None, &credentials, "").await;
    assert_eq!(status, 200);
    assert_eq!(original, bytes);
    assert_eq!(
        fixture
            .request("PUT", canon, None, &credentials, "different")
            .await
            .0,
        412
    );
    assert_eq!(
        fixture
            .request("HEAD", source, None, &credentials, "")
            .await
            .1["content-length"],
        bytes.len().to_string()
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                source,
                None,
                &[auth.clone(), ("Range", "bytes=1-2".into())],
                ""
            )
            .await
            .2,
        &bytes[1..3]
    );

    let rename = "/dav/Files/renamed.bin";
    let collision = "/dav/Files/private-file.bin";
    for (destination, expected) in [
        (
            "https://foreign.invalid/dav/Files/renamed.bin".to_owned(),
            502,
        ),
        ("//foreign.invalid/dav/Files/renamed.bin".to_owned(), 400),
        (format!("{origin}/dav/Files/renamed.bin?query=yes"), 400),
        (collision.to_owned(), 412),
    ] {
        assert_eq!(
            fixture
                .request(
                    "MOVE",
                    source,
                    None,
                    &[
                        auth.clone(),
                        ("Destination", destination),
                        ("Overwrite", "T".into())
                    ],
                    ""
                )
                .await
                .0,
            expected
        );
    }
    assert_eq!(
        fixture
            .request(
                "MOVE",
                source,
                None,
                &[
                    auth.clone(),
                    ("Destination", rename.into()),
                    ("If-Match", "\"stale\"".into())
                ],
                ""
            )
            .await
            .0,
        412
    );
    assert_eq!(
        fixture
            .request(
                "MOVE",
                source,
                None,
                &[
                    auth.clone(),
                    ("Destination", format!("{origin}{rename}")),
                    ("If-Match", tag.clone()),
                    ("Overwrite", "F".into())
                ],
                ""
            )
            .await
            .0,
        201
    );
    assert_eq!(
        fixture
            .request("GET", source, None, &credentials, "")
            .await
            .0,
        404
    );
    let moved = format!("{folder}/arrived.bin");
    assert_eq!(
        fixture
            .request(
                "MOVE",
                rename,
                None,
                &[auth.clone(), ("Destination", moved.clone())],
                ""
            )
            .await
            .0,
        201
    );
    assert_eq!(
        fixture
            .request("GET", rename, None, &credentials, "")
            .await
            .0,
        404
    );
    let (status, headers, bytes_after_move) =
        fixture.request("GET", &moved, None, &credentials, "").await;
    assert_eq!(status, 200);
    assert_eq!(headers["etag"], tag);
    assert_eq!(Sha256::digest(bytes_after_move), Sha256::digest(bytes));

    let (id,): (String,) = fixture.workers.as_ref().unwrap().read(|db| {
        db.query_row("SELECT entry_id FROM entry WHERE owner_account_id=?1 AND original_name='arrived.bin'", [OWNER], |row| Ok((row.get(0)?,)))
    }).await.unwrap();
    let (status, _, share) = fixture
        .request(
            "POST",
            "/share-links",
            cookie(0),
            &auth_headers,
            &serde_json::json!({ "entry_id": id, "expiry_days": 1 }).to_string(),
        )
        .await;
    assert_eq!(status, 201);
    let share: serde_json::Value = serde_json::from_slice(&share).unwrap();
    let link = share["url"]
        .as_str()
        .unwrap()
        .strip_prefix(&origin)
        .unwrap();
    assert_eq!(fixture.request("GET", link, None, &[], "").await.0, 200);
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &moved,
                None,
                &[
                    auth.clone(),
                    ("If", "(<opaquelocktoken:unsupported>)".into())
                ],
                ""
            )
            .await
            .0,
        412
    );
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &moved,
                None,
                &[auth.clone(), ("If-Match", tag)],
                ""
            )
            .await
            .0,
        204
    );
    assert_eq!(
        fixture
            .request("GET", &moved, None, &credentials, "")
            .await
            .0,
        404
    );
    assert_eq!(fixture.request("GET", link, None, &[], "").await.0, 404);
    let (status, _, trash) = fixture.request("GET", "/trash", cookie(0), &[], "").await;
    assert_eq!(status, 200);
    let trash: serde_json::Value = serde_json::from_slice(&trash).unwrap();
    let row = trash["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap();
    let restore_tag = row["etag"].as_str().unwrap();
    let mut restore_headers = auth_headers.to_vec();
    restore_headers.push(("If-Match", restore_tag.into()));
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/trash/{id}/restore"),
                cookie(0),
                &restore_headers,
                ""
            )
            .await
            .0,
        204
    );
    assert_eq!(
        fixture
            .request("GET", &moved, None, &credentials, "")
            .await
            .2,
        bytes
    );
    fixture.close().await;
}

#[tokio::test]
async fn webdav_upload_rechecks_path_and_revocation_at_final_commit() {
    async fn wait_at_commit(fixture: &Fixture) {
        tokio::time::timeout(Duration::from_secs(5), fixture.upload_commit.wait_paused())
            .await
            .unwrap();
    }

    let fixture = Fixture::with_webdav(true).await;
    let (id, writer) = fixture.credential(0, "Primary", false).await;
    let (_, organizer) = fixture.credential(0, "Organizer", false).await;
    let path = "/dav/Files/private-name/nested/";
    assert_eq!(
        fixture
            .request("MKCOL", path, None, std::slice::from_ref(&writer), "")
            .await
            .0,
        201
    );
    let target = format!("{path}result.bin");
    let target_after_move = "/dav/Files/changed/nested/result.bin";

    let writer_headers = [writer];
    fixture.upload_commit.arm();
    let (upload, ()) = tokio::join!(
        fixture.request("PUT", &target, None, &writer_headers, "first"),
        async {
            wait_at_commit(&fixture).await;
            let response = fixture
                .request(
                    "MOVE",
                    "/dav/Files/private-name/",
                    None,
                    &[
                        organizer.clone(),
                        ("Destination", "/dav/Files/changed/".into()),
                    ],
                    "",
                )
                .await;
            fixture.upload_commit.release();
            assert_eq!(response.0, 201);
        }
    );
    assert_eq!(
        upload.0, 412,
        "a renamed ancestor must invalidate the DAV path"
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                target_after_move,
                None,
                std::slice::from_ref(&organizer),
                ""
            )
            .await
            .0,
        404
    );

    fixture.upload_commit.arm();
    let (upload, ()) = tokio::join!(
        fixture.request("PUT", target_after_move, None, &writer_headers, "second"),
        async {
            wait_at_commit(&fixture).await;
            let response = fixture
                .request(
                    "DELETE",
                    &format!("/me/webdav/{id}"),
                    cookie(0),
                    &fixture.mutation_headers(CSRF),
                    "",
                )
                .await;
            fixture.upload_commit.release();
            assert_eq!(response.0, 204);
        }
    );
    assert_eq!(upload.0, 401);
    assert_eq!(
        fixture
            .request(
                "GET",
                target_after_move,
                None,
                std::slice::from_ref(&organizer),
                ""
            )
            .await
            .0,
        404
    );
    assert_eq!(
        fixture
            .request(
                "PUT",
                target_after_move,
                None,
                std::slice::from_ref(&organizer),
                "accepted"
            )
            .await
            .0,
        201
    );
    assert_eq!(
        fixture
            .request(
                "PUT",
                target_after_move,
                None,
                std::slice::from_ref(&organizer),
                "retry"
            )
            .await
            .0,
        412
    );
    assert_eq!(
        fixture
            .request("GET", target_after_move, None, &[organizer], "")
            .await
            .2,
        b"accepted"
    );
    let settled = fixture
        .workers
        .as_ref()
        .unwrap()
        .read(|db| {
            db.query_row(
                "SELECT count(*) FROM mutation m JOIN upload_attempt a USING (mutation_id)
            WHERE m.state='rejected' AND m.result_revision_id IS NULL AND a.outcome='canceled'
              AND a.writer_active=0 AND m.completed_sha256=a.completed_sha256",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(settled, 2);
    fixture.close().await;
}

#[tokio::test]
async fn webdav_lost_put_response_does_not_duplicate_a_publication() {
    use super::common::server::tls_connect;
    use tokio::io::AsyncWriteExt;

    let fixture = Fixture::with_webdav(true).await;
    let (_, credential) = fixture.credential(0, "Lost response", false).await;
    let path = "/dav/Files/lost-response.bin";
    let address = fixture.server.as_ref().unwrap().address();
    let mut socket = tls_connect(address, &fixture.ca, "127.0.0.1")
        .await
        .unwrap();
    fixture.upload_commit.arm();
    let request = format!(
        "PUT {path} HTTP/1.1\r\nHost: {address}\r\n{}: {}\r\nContent-Length: 8\r\n\r\nsurvives",
        credential.0, credential.1
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.upload_commit.wait_paused())
        .await
        .unwrap();
    drop(socket);
    fixture.upload_commit.release();

    let auth = [credential];
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (status, _, payload) = fixture.request("GET", path, None, &auth, "").await;
            if status == 200 {
                assert_eq!(payload, b"survives");
                break;
            }
            assert_eq!(status, 404);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture.request("PUT", path, None, &auth, "retry").await.0,
        412
    );
    let published = fixture.workers.as_ref().unwrap().read(|db| {
        db.query_row(
            "SELECT count(*) FROM mutation m JOIN revision r ON r.revision_id=m.result_revision_id
             JOIN entry e ON e.entry_id=r.entry_id
             WHERE m.principal_account_id=?1 AND m.state='committed' AND e.original_name='lost-response.bin'",
            [OWNER], |row| row.get::<_, i64>(0),
        )
    }).await.unwrap();
    assert_eq!(published, 1);
    fixture.close().await;
}

#[tokio::test]
async fn webdav_in_flight_put_allows_a_sibling_but_rejects_a_destination_collision() {
    use super::common::server::tls_connect;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let fixture = Fixture::with_webdav(true).await;
    let (_, credential) = fixture.credential(0, "Concurrent writes", false).await;
    let auth = [credential];
    let address = fixture.server.as_ref().unwrap().address();
    let slow = "/dav/Files/slow.bin";
    let fast = "/dav/Files/fast.bin";

    for (target, concurrent_target, expected) in [
        (slow, fast, 201),
        ("/dav/Files/collision.bin", "/dav/Files/collision.bin", 412),
    ] {
        let mut socket = tls_connect(address, &fixture.ca, "127.0.0.1")
            .await
            .unwrap();
        let request = format!(
            "PUT {target} HTTP/1.1\r\nHost: {address}\r\n{}: {}\r\nConnection: close\r\nContent-Length: 8\r\n\r\nslow",
            auth[0].0, auth[0].1
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let workers = fixture.workers.as_ref().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let active = workers
                    .read(|db| {
                        db.query_row(
                            "SELECT EXISTS(SELECT 1 FROM upload_attempt WHERE writer_active=1)",
                            [],
                            |row| row.get::<_, bool>(0),
                        )
                    })
                    .await
                    .unwrap();
                if active {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            fixture
                .request("PUT", concurrent_target, None, &auth, "fast bytes")
                .await
                .0,
            201
        );
        socket.write_all(b"body").await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert!(
            reply.starts_with(format!("HTTP/1.1 {expected} ").as_bytes()),
            "{:?}",
            String::from_utf8_lossy(&reply)
        );
        let (status, _, bytes) = fixture
            .request("GET", concurrent_target, None, &auth, "")
            .await;
        assert_eq!(status, 200);
        assert_eq!(bytes, b"fast bytes");
    }

    let (status, _, bytes) = fixture.request("GET", slow, None, &auth, "").await;
    assert_eq!(status, 200);
    assert_eq!(bytes, b"slowbody");
    fixture.close().await;
}

#[tokio::test]
async fn webdav_settles_a_committed_put_when_the_sqlite_result_is_lost() {
    use sylux::domain::upload::revision_etag;

    let fixture = Fixture::with_webdav(true).await;
    let (_, credential) = fixture.credential(0, "Lost commit result", false).await;
    let auth = [credential];
    let path = "/dav/Files/lost-commit-result.bin";

    fixture.upload_commit.lose_next_committed_result();
    let (status, committed, _) = fixture
        .request("PUT", path, None, &auth, "original bytes")
        .await;
    assert!(!fixture.upload_commit.result_loss_pending());
    assert_eq!(status, 201);
    assert_eq!(
        fixture.request("PUT", path, None, &auth, "retry").await.0,
        412
    );
    let (status, downloaded, bytes) = fixture.request("GET", path, None, &auth, "").await;
    assert_eq!(status, 200);
    assert_eq!(bytes, b"original bytes");

    let results = fixture
        .workers
        .as_ref()
        .unwrap()
        .read(|db| {
            let mut statement = db.prepare(
                "SELECT r.revision_id, m.state, m.result_revision_id
             FROM revision r JOIN entry e ON e.entry_id=r.entry_id
             LEFT JOIN mutation m ON m.result_revision_id=r.revision_id
             WHERE e.owner_account_id=?1 AND e.original_name='lost-commit-result.bin'",
            )?;
            statement
                .query_map([OWNER], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    let [(revision, state, result)] = results.as_slice() else {
        panic!("expected exactly one published revision, got {results:?}");
    };
    assert_eq!(state, "committed");
    assert_eq!(revision, result);
    assert_eq!(committed["etag"], revision_etag(revision));
    assert_eq!(downloaded["etag"], committed["etag"]);
    fixture.close().await;
}

#[tokio::test]
async fn webdav_respects_direct_grant_boundaries_on_reads_moves_and_trash() {
    let fixture = Fixture::with_webdav(true).await;
    let (_, owner) = fixture.credential(0, "Owner desktop", false).await;
    let (_, recipient) = fixture.credential(1, "Recipient desktop", false).await;
    let (_, unrelated) = fixture
        .credential(2, "Unrelated administrator", false)
        .await;
    let headers = fixture.mutation_headers(CSRF);
    let folder = "00000000000000000000000000000301";
    let file = "00000000000000000000000000000302";
    let original_folder = "/dav/Files/private-name/";
    let original_child = "/dav/Files/private-name/shared.bin";
    let renamed_folder = "/dav/Files/renamed/";
    let renamed_child = "/dav/Files/renamed/shared.bin";
    let staging = "/dav/Files/staging/";
    let moved_child = "/dav/Files/staging/moved.bin";
    let new_upload = "/dav/Files/renamed/upload.bin";
    let folder_bookmark = format!("/shared/folders/{folder}/path");
    let shared_content = format!("/shared/entries/{file}/content");
    let owned_content = format!("/entries/{file}/content");
    let dav = async |method, path, body| {
        fixture
            .request(method, path, None, std::slice::from_ref(&owner), body)
            .await
    };
    let move_to = async |source, target: &str, overwrite: &str| {
        fixture
            .request(
                "MOVE",
                source,
                None,
                &[
                    owner.clone(),
                    ("Destination", target.into()),
                    ("Overwrite", overwrite.into()),
                ],
                "",
            )
            .await
            .0
    };
    let browser_get =
        async |path, actor| fixture.request("GET", path, cookie(actor), &[], "").await;

    assert_eq!(
        move_to("/dav/Files/private-file.bin", original_child, "F").await,
        201
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/entries/{folder}/direct-grants"),
                cookie(0),
                &headers,
                r#"{"handle":"other"}"#
            )
            .await
            .0,
        200
    );

    for (actor, auth) in [&owner, &recipient, &unrelated].into_iter().enumerate() {
        for (method, path, owner_status) in [
            ("GET", original_child, 200),
            ("HEAD", original_child, 200),
            ("PROPFIND", original_folder, 207),
        ] {
            assert_eq!(
                fixture
                    .request(
                        method,
                        path,
                        None,
                        &[auth.clone(), ("Depth", "0".into())],
                        ""
                    )
                    .await
                    .0,
                if actor == 0 { owner_status } else { 404 },
                "{method}, account {actor}"
            );
        }
        assert_eq!(
            browser_get(&owned_content, actor).await.0,
            if actor == 0 { 200 } else { 404 }
        );
        assert_eq!(
            browser_get(&shared_content, actor).await.0,
            if actor == 1 { 200 } else { 404 }
        );
    }

    assert_eq!(move_to(original_folder, renamed_folder, "F").await, 201);
    assert_eq!(move_to(renamed_folder, renamed_folder, "T").await, 204);
    assert_eq!(browser_get(&folder_bookmark, 1).await.0, 200);
    assert_eq!(dav("MKCOL", staging, "").await.0, 201);
    assert_eq!(
        dav("MKCOL", "/dav/Files/renamed/subfolder/", "").await.0,
        201
    );
    assert_eq!(dav("PUT", new_upload, "new bytes").await.0, 201);
    assert_eq!(move_to(renamed_child, moved_child, "F").await, 201);
    assert_eq!(browser_get(&shared_content, 1).await.0, 404);

    for overwrite in ["F", "T"] {
        for (source, destination, status) in [
            (moved_child, renamed_child, 403),
            (renamed_folder, "/dav/Files/staging/renamed/", 403),
            (moved_child, new_upload, 412),
        ] {
            assert_eq!(
                move_to(source, destination, overwrite).await,
                status,
                "{source} -> {destination}, Overwrite: {overwrite}"
            );
        }
    }
    assert_eq!(dav("GET", renamed_child, "").await.0, 404);
    assert_eq!(
        fixture
            .request(
                "PROPFIND",
                "/dav/Files/staging/renamed/",
                None,
                &[owner.clone(), ("Depth", "0".into())],
                ""
            )
            .await
            .0,
        404
    );
    assert_eq!(
        dav("GET", moved_child, "").await.2,
        b"synthetic isolation payload"
    );
    assert_eq!(dav("GET", new_upload, "").await.2, b"new bytes");
    assert_eq!(browser_get(&folder_bookmark, 1).await.0, 200);
    assert_eq!(browser_get(&shared_content, 1).await.0, 404);

    assert_eq!(dav("DELETE", renamed_folder, "").await.0, 204);
    assert_eq!(browser_get(&folder_bookmark, 1).await.0, 404);
    let (status, _, trash) = browser_get("/trash", 0).await;
    assert_eq!(status, 200);
    let trash: serde_json::Value = serde_json::from_slice(&trash).unwrap();
    let row = trash["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == folder)
        .unwrap();
    let mut restore_headers = headers.to_vec();
    restore_headers.push(("If-Match", row["etag"].as_str().unwrap().into()));
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/trash/{folder}/restore"),
                cookie(0),
                &restore_headers,
                ""
            )
            .await
            .0,
        204
    );
    assert_eq!(dav("GET", new_upload, "").await.2, b"new bytes");
    assert_eq!(browser_get(&folder_bookmark, 1).await.0, 404);
    let (status, _, incoming) = browser_get("/shared", 1).await;
    assert_eq!(status, 200);
    let incoming: serde_json::Value = serde_json::from_slice(&incoming).unwrap();
    assert_eq!(incoming, serde_json::json!({"items":[]}));
    fixture.close().await;
}

#[tokio::test]
async fn webdav_publication_preserves_owner_reads_and_blocks_unconfirmed_exposure() {
    use serde_json::Value;

    let f = Fixture::with_webdav(true).await;
    let (_, owner) = f.credential(0, "Publication desktop", false).await;
    let (_, recipient) = f.credential(1, "Recipient desktop", false).await;
    let mutation = f.mutation_headers(CSRF);
    let (status, _, body) = f
        .request(
            "POST",
            "/libraries",
            cookie(0),
            &mutation,
            r#"{"name":"Published"}"#,
        )
        .await;
    assert_eq!(status, 201);
    let root: Value = serde_json::from_slice(&body).unwrap();
    let root = root["id"].as_str().unwrap();
    let publish_path = format!("/libraries/{root}/publication");
    let held = "/dav/Published/held.bin";
    let sibling = "/dav/Files/private-file.bin";
    let published = "/dav/Published/read.bin";
    let moved_out = "/dav/Files/moved-out.bin";
    let bookmark = "/shared/entries/00000000000000000000000000000302/content";
    let dav = async |method, path, body| {
        f.request(method, path, None, std::slice::from_ref(&owner), body)
            .await
    };
    let move_to = async |from, to: &str| {
        f.request(
            "MOVE",
            from,
            None,
            &[owner.clone(), ("Destination", to.into())],
            "",
        )
        .await
        .0
    };
    let read = async || f.request("GET", bookmark, cookie(1), &[], "").await;

    assert_eq!(move_to(sibling, published).await, 201);
    assert_eq!(read().await.0, 404);

    f.upload_commit.arm();
    let (upload, (status, _, publication)) =
        tokio::join!(dav("PUT", held, "unconfirmed private bytes"), async {
            tokio::time::timeout(Duration::from_secs(5), f.upload_commit.wait_paused())
                .await
                .unwrap();
            let response = f
                .request("POST", &publish_path, cookie(0), &mutation, "")
                .await;
            f.upload_commit.release();
            response
        });
    assert_eq!(status, 200);
    assert_eq!(upload.0, 403);
    assert_eq!(dav("GET", held, "").await.0, 404);

    assert_eq!(read().await.2, b"synthetic isolation payload");
    assert_eq!(
        dav("GET", published, "").await.2,
        b"synthetic isolation payload"
    );
    assert_eq!(
        f.request("GET", published, None, std::slice::from_ref(&recipient), "")
            .await
            .0,
        404
    );
    let (status, _, xml) = f
        .request(
            "PROPFIND",
            "/dav/",
            None,
            &[recipient, ("Depth", "1".into())],
            "",
        )
        .await;
    assert_eq!(status, 207);
    assert!(!std::str::from_utf8(&xml).unwrap().contains("Published"));
    assert_eq!(dav("MKCOL", "/dav/Published/new/", "").await.0, 403);
    assert_eq!(
        dav("PUT", "/dav/Published/new.bin", "new bytes").await.0,
        403
    );
    assert_eq!(move_to(published, moved_out).await, 201);
    assert_eq!(read().await.0, 404);
    assert_eq!(move_to(moved_out, published).await, 403);
    assert_eq!(
        dav("GET", moved_out, "").await.2,
        b"synthetic isolation payload"
    );
    assert_eq!(read().await.0, 404);

    let state: Value = serde_json::from_slice(&publication).unwrap();
    let withdrawal = format!("/library-publications/{}", state["id"].as_str().unwrap());
    assert_eq!(
        f.request("DELETE", &withdrawal, cookie(0), &mutation, "")
            .await
            .0,
        204
    );
    assert_eq!(move_to(moved_out, published).await, 201);
    assert_eq!(dav("PUT", held, "now private").await.0, 201);
    assert_eq!(read().await.0, 404);
    f.close().await;
}

#[tokio::test]
async fn expired_folder_link_does_not_require_confirmation_or_block_dav() {
    use serde_json::{Value, json};

    let f = Fixture::with_webdav(true).await;
    let (_, owner) = f.credential(0, "Public link desktop", false).await;
    let mutation = f.mutation_headers(CSRF);
    let (status, _, body) = f
        .request(
            "POST",
            "/libraries",
            cookie(0),
            &mutation,
            r#"{"name":"Recipient folder"}"#,
        )
        .await;
    assert_eq!(status, 201);
    let root: Value = serde_json::from_slice(&body).unwrap();
    let share = json!({"entry_id":root["id"]}).to_string();
    let dav = async |method, path, body| {
        f.request(method, path, None, std::slice::from_ref(&owner), body)
            .await
    };
    let (status, _, body) = f
        .request("POST", "/share-links", cookie(0), &mutation, &share)
        .await;
    assert_eq!(status, 201);
    let link = serde_json::from_slice::<Value>(&body).unwrap();

    let link_id = link["id"].as_str().unwrap();
    let expired_id = link_id.to_owned();
    let workers = f.workers.as_ref().unwrap();
    workers
        .transact(move |tx| {
            assert_eq!(
                tx.execute(
                    "UPDATE share_link SET created_at=1,expires_at=86401 WHERE share_id=?1",
                    [expired_id],
                )?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();

    let folder = root["id"].as_str().unwrap();
    let (status, _, body) = f
        .request(
            "GET",
            &format!("/folders/{folder}/path?public_links=true"),
            cookie(0),
            &[],
            "",
        )
        .await;
    assert_eq!(status, 200);
    let path: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(path["public_link_active"], false);

    let file = "00000000000000000000000000000302";
    let mut move_headers = mutation.to_vec();
    move_headers.push(("If-Match", format!("\"{file}:1\"")));
    let (status, _, _) = f
        .request(
            "POST",
            &format!("/entries/{file}/move"),
            cookie(0),
            &move_headers,
            &json!({"parent_id": folder}).to_string(),
        )
        .await;
    assert_eq!(
        status, 200,
        "an expired link does not require move confirmation"
    );
    assert_eq!(
        dav("PUT", "/dav/Recipient%20folder/expired.bin", "after expiry")
            .await
            .0,
        201
    );
    assert_eq!(
        dav("GET", "/dav/Recipient%20folder/expired.bin", "")
            .await
            .2,
        b"after expiry"
    );
    let expired_id = link_id.to_owned();
    let persisted: i64 = workers
        .read(move |db| {
            db.query_row(
                "SELECT count(*) FROM share_link WHERE share_id=?1 AND expires_at=86401",
                [expired_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        persisted, 1,
        "the audience checks ran before expired-link cleanup"
    );

    f.close().await;
}

#[tokio::test]
async fn webdav_sync_updates_a_live_public_folder_without_granting_write_access() {
    use serde_json::{Value, json};

    fn entry_href<'a>(html: &'a [u8], label: &str) -> &'a str {
        let html = std::str::from_utf8(html).unwrap();
        let prefix = html.split_once(&format!("<span>{label}</span>")).unwrap().0;
        prefix
            .rsplit_once("href=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap()
    }

    let f = Fixture::with_webdav(true).await;
    let (_, owner) = f.credential(0, "Public link desktop", false).await;
    let (_, reader) = f.credential(0, "DAV reader", true).await;
    let (_, other) = f.credential(1, "Other DAV writer", false).await;
    let mutation = f.mutation_headers(CSRF);
    let (status, _, body) = f
        .request(
            "POST",
            "/libraries",
            cookie(0),
            &mutation,
            r#"{"name":"Recipient folder"}"#,
        )
        .await;
    assert_eq!(status, 201);
    let root: Value = serde_json::from_slice(&body).unwrap();
    let share = json!({"entry_id": root["id"]}).to_string();
    let dav = async |method, path, body| {
        f.request(method, path, None, std::slice::from_ref(&owner), body)
            .await
    };
    let browse = async |path| {
        let (status, _, body) = f.request("GET", path, None, &[], "").await;
        assert_eq!(status, 200);
        body
    };

    let pending = "/dav/Recipient%20folder/pending.bin";
    f.upload_commit.arm();
    let (put, (status, _, body)) = tokio::join!(dav("PUT", pending, "pending bytes"), async {
        tokio::time::timeout(Duration::from_secs(5), f.upload_commit.wait_paused())
            .await
            .unwrap();
        let issued = f
            .request("POST", "/share-links", cookie(0), &mutation, &share)
            .await;
        f.upload_commit.release();
        issued
    });
    assert_eq!(status, 201);
    assert_eq!(put.0, 201);
    let link: Value = serde_json::from_slice(&body).unwrap();
    let origin = f.dav_origin();
    let root_url = link["url"].as_str().unwrap().strip_prefix(&origin).unwrap();
    let html = browse(root_url).await;
    let pending_url = entry_href(&html, "pending.bin");
    assert_eq!(browse(pending_url).await, b"pending bytes");

    let directory = "/dav/Recipient%20folder/new/";
    let file = "/dav/Recipient%20folder/new/fresh.bin";
    assert_eq!(dav("MKCOL", directory, "").await.0, 201);
    let added = dav("PUT", file, "original bytes").await;
    assert_eq!(added.0, 201);
    let etag = added.1["etag"].clone();
    let html = browse(root_url).await;
    let folder_url = entry_href(&html, "new");
    let old_listing = browse(folder_url).await;
    let old_url = entry_href(&old_listing, "fresh.bin");
    assert_eq!(browse(old_url).await, b"original bytes");

    for (credentials, expected) in [(vec![], 401), (vec![reader], 403), (vec![other], 409)] {
        assert_eq!(
            f.request("PUT", file, None, &credentials, "denied").await.0,
            expected
        );
        assert_eq!(
            f.request(
                "MKCOL",
                "/dav/Recipient%20folder/denied/",
                None,
                &credentials,
                ""
            )
            .await
            .0,
            expected
        );
    }
    assert_eq!(dav("PUT", file, "unqualified overwrite").await.0, 412);
    let conditional = [owner.clone(), ("If-Match", etag)];
    let replaced = f
        .request("PUT", file, None, &conditional, "replacement bytes")
        .await;
    assert_eq!(replaced.0, 204);
    assert_ne!(replaced.1["etag"], conditional[1].1);
    assert_eq!(
        f.request("PUT", file, None, &conditional, "stale overwrite")
            .await
            .0,
        412
    );
    assert_eq!(f.request("GET", old_url, None, &[], "").await.0, 404);
    let new_listing = browse(folder_url).await;
    let new_url = entry_href(&new_listing, "fresh.bin");
    assert_eq!(browse(new_url).await, b"replacement bytes");

    let existing = "/dav/Files/private-file.bin";
    assert_eq!(
        f.request(
            "MOVE",
            existing,
            None,
            &[
                owner.clone(),
                ("Destination", "/dav/Recipient%20folder/moved.bin".into())
            ],
            ""
        )
        .await
        .0,
        403
    );
    assert_eq!(dav("GET", existing, "").await.0, 200);
    let revoke = format!("/share-links/{}", link["id"].as_str().unwrap());
    assert_eq!(
        f.request("DELETE", &revoke, cookie(0), &mutation, "")
            .await
            .0,
        204
    );
    for target in [root_url, folder_url, pending_url, new_url] {
        assert_eq!(f.request("GET", target, None, &[], "").await.0, 404);
    }
    assert_eq!(dav("GET", file, "").await.2, b"replacement bytes");
    f.close().await;
}
