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
        .request("OPTIONS", path, None, &[owner.clone()], "")
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
                    &[credential.clone()],
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
            .request("GET", source, None, &[owner.clone()], "")
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
            .request("OPTIONS", "/dav/", None, &[other.clone()], "")
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
            .request("MKCOL", path, None, &[writer.clone()], "")
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
            .request("GET", target_after_move, None, &[organizer.clone()], "")
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
            .request("GET", target_after_move, None, &[organizer.clone()], "")
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
                &[organizer.clone()],
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
                &[organizer.clone()],
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
