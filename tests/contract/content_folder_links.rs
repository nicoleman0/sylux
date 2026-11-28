use super::*;

async fn issue_folder_link(f: &Fixture, folder: &str, days: Option<u8>) -> serde_json::Value {
    let mut input = serde_json::json!({"entry_id": folder});
    if let Some(days) = days {
        input["expiry_days"] = days.into();
    }
    let (status, reply) = f
        .share_request("POST", "/share-links", GRANT_OWNER_ACTOR, input, None, None)
        .await;
    assert_eq!(status, 201);
    assert_eq!(reply["kind"], "folder");
    reply
}

fn folder_link_base(link: &serde_json::Value) -> &str {
    public_path(link).strip_suffix("/browse").unwrap()
}

async fn folder_tag(f: &Fixture, folder: &str) -> String {
    f.share_get(&format!("/folders/{folder}/path"), GRANT_OWNER_ACTOR)
        .await
        .1["items"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["etag"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn revoke_public_link(f: &Fixture, link: &serde_json::Value) {
    let status = f
        .share_request(
            "DELETE",
            &format!("/share-links/{}", link["id"].as_str().unwrap()),
            GRANT_OWNER_ACTOR,
            serde_json::Value::Null,
            None,
            None,
        )
        .await
        .0;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn public_folder_link_follows_the_live_tree_and_never_exposes_ancestors() {
    use serde_json::{Value, json};
    use sylux::domain::upload::folder_etag;

    let f = Fixture::new("public-folder-tree").await;
    let ancestor = f.share_folder(ROOT, "Private ancestor").await;
    let private = f.share_folder(ROOT, "Private destination").await;
    let root = f.share_folder(&ancestor, "Shared <project>").await;
    let nested = f.share_folder(&root, "Nested").await;
    let spare = f.share_folder(&root, "Remaining contents").await;
    let other_folder = f.share_folder(&private, "Private sibling").await;
    let move_to = async |id: &str, parent: &str, tag: &str, token: Option<&str>| {
        f.share_request(
            "POST",
            &format!("/entries/{id}/move"),
            GRANT_OWNER_ACTOR,
            json!({"parent_id": parent}),
            Some(tag),
            token,
        )
        .await
    };
    let (status, moved) = move_to(SHARE_FILE, &nested, &folder_etag(SHARE_FILE, 1), None).await;
    assert_eq!(status, 200);
    let file_tag = moved["etag"].as_str().unwrap();
    let (_, standalone) = issue_share(&f, Some(30)).await;
    let folder = issue_folder_link(&f, &root, None).await;
    assert_eq!(
        folder["expires_at"].as_i64().unwrap() - folder["created_at"].as_i64().unwrap(),
        604_800
    );
    let base = folder_link_base(&folder);
    let browse = public_path(&folder);
    let nested_page = format!("{base}/folders/{nested}");
    let spare_page = format!("{base}/folders/{spare}");
    let saved = format!("{base}/files/{SHARE_FILE}");
    let (status, headers, body) = f.request("GET", browse, None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(!headers.contains_key("set-cookie"));
    assert!(headers["content-security-policy"].contains("default-src 'none'"));
    let body = String::from_utf8(body).unwrap();
    for visible in ["Shared &lt;project&gt;", "Remaining contents", "Nested"] {
        assert!(body.contains(visible));
    }
    for private_value in [
        "Private ancestor",
        "Private destination",
        "Private sibling",
        &ancestor,
        &private,
        OWNER,
        OTHER_ROOT,
    ] {
        assert!(!body.contains(private_value));
    }
    let (status, _, body) = f.request("GET", &nested_page, None, &[]).await;
    assert_eq!(status, 200);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("unsafe &lt;name&gt;.bin"));
    assert!(body.contains("Shared &lt;project&gt;"));
    assert!(body.contains(&saved));
    assert!(!body.contains(&ancestor));
    let (status, headers, bytes) = f.request("GET", &saved, None, &[]).await;
    assert_eq!((status, bytes.as_slice()), (200, BINARY_FILE.1));
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert!(headers["content-disposition"].starts_with("attachment;"));
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["cache-control"], "no-store");

    for path in [
        format!("{base}/folders/{ancestor}"),
        format!("{base}/folders/{other_folder}"),
        format!("{base}/folders/{OTHER_ROOT}"),
        format!("{base}/files/{nested}"),
        format!("{base}/search"),
        format!("{base}/preview/{SHARE_FILE}"),
        format!("{base}/archive"),
        format!("{browse}?q=anything"),
        format!("{browse}?after={other_folder}"),
        format!("{}/browse", public_path(&standalone)),
        base.to_owned(),
    ] {
        assert_share_missing(&f, &path).await;
        let (status, _, body) = f
            .request("GET", &path, Some(auth(SESSION, SECRET)), &[])
            .await;
        assert_eq!(status, 404);
        assert_eq!(body, br#"{"code":"share_not_found"}"#);
    }
    for path in [
        format!("/entries/{SHARE_FILE}/content"),
        "/entries/search?q=unsafe".to_owned(),
        "/share-links".to_owned(),
        "/me".to_owned(),
    ] {
        assert_eq!(f.request("GET", &path, None, &[]).await.0, 401);
    }
    assert_eq!(f.request("POST", browse, None, &[]).await.0, 405);

    let added = f.share_folder(&nested, "Added later").await;
    assert_eq!(
        f.request("GET", &format!("{base}/folders/{added}"), None, &[])
            .await
            .0,
        200
    );
    assert_eq!(
        f.share_get(
            &format!("/folders/{nested}/path?public_links=true"),
            GRANT_OWNER_ACTOR
        )
        .await
        .1["public_link_active"],
        true
    );
    assert_eq!(
        f.share_get(
            &format!("/folders/{nested}/path?public_links=true"),
            GRANT_RECIPIENT_ACTOR
        )
        .await
        .0,
        404
    );

    let (status, moved_out) = move_to(SHARE_FILE, &private, file_tag, None).await;
    assert_eq!(status, 200);
    assert_share_missing(&f, &saved).await;
    assert_eq!(
        f.request("GET", public_path(&standalone), None, &[])
            .await
            .0,
        200
    );
    let tag = moved_out["etag"].as_str().unwrap();
    let (status, challenge) = move_to(SHARE_FILE, &nested, tag, None).await;
    assert_eq!(status, 409);
    assert_eq!(challenge["confirmation"]["exposes_to_public_links"], true);
    assert_share_missing(&f, &saved).await;
    let (status, _) = move_to(
        SHARE_FILE,
        &nested,
        tag,
        challenge["confirmation"]["token"].as_str(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(f.request("GET", &saved, None, &[]).await.0, 200);

    let tag = folder_tag(&f, &nested).await;
    let (status, moved) = move_to(&nested, &private, &tag, None).await;
    assert_eq!(status, 200);
    assert_share_missing(&f, &nested_page).await;
    assert_share_missing(&f, &saved).await;
    assert_eq!(f.request("GET", &spare_page, None, &[]).await.0, 200);
    let tag = moved["etag"].as_str().unwrap();
    let (status, challenge) = move_to(&nested, &root, tag, None).await;
    assert_eq!(status, 409);
    assert_share_missing(&f, &nested_page).await;
    // A link created after the first acknowledgement requires new approval.
    let added_link = issue_folder_link(&f, &root, Some(1)).await;
    let (status, challenge) = move_to(
        &nested,
        &root,
        tag,
        challenge["confirmation"]["token"].as_str(),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(challenge["confirmation"]["exposes_to_public_links"], true);
    assert_eq!(
        move_to(
            &nested,
            &root,
            tag,
            challenge["confirmation"]["token"].as_str()
        )
        .await
        .0,
        200
    );

    let rename = f
        .share_request(
            "PATCH",
            &format!("/entries/{root}"),
            GRANT_OWNER_ACTOR,
            json!({"name":"Renamed & shared"}),
            Some(&folder_tag(&f, &root).await),
            None,
        )
        .await;
    assert_eq!(rename.0, 200);
    assert_eq!(
        move_to(&root, &private, rename.1["etag"].as_str().unwrap(), None)
            .await
            .0,
        200
    );
    let (status, _, body) = f.request("GET", &nested_page, None, &[]).await;
    assert_eq!(status, 200);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("Renamed &amp; shared"));
    assert!(!body.contains("Private destination"));
    assert!(!body.contains(&private));

    for (target, public_root) in [(&nested, false), (&root, true), (&private, true)] {
        let current_link = issue_folder_link(&f, &root, Some(1)).await;
        let current_download = format!("{}/files/{SHARE_FILE}", folder_link_base(&current_link));
        let tag = folder_tag(&f, target).await;
        assert_eq!(
            f.share_request(
                "DELETE",
                &format!("/entries/{target}"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                Some(&tag),
                None
            )
            .await
            .0,
            204
        );
        assert_share_missing(&f, &current_download).await;
        let restore = format!("/trash/{target}/restore");
        let trash = f.share_get("/trash", GRANT_OWNER_ACTOR).await.1;
        let deleted = trash["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == target.as_str())
            .unwrap();
        let tag = deleted["etag"].as_str();
        if public_root {
            assert_share_missing(&f, public_path(&current_link)).await;
            assert_eq!(
                f.share_request("POST", &restore, GRANT_OWNER_ACTOR, Value::Null, tag, None)
                    .await
                    .0,
                204
            );
            assert_share_missing(&f, public_path(&current_link)).await;
            assert_share_missing(&f, &current_download).await;
        } else {
            assert_eq!(f.request("GET", &spare_page, None, &[]).await.0, 200);
            let (status, challenge) = f
                .share_request("POST", &restore, GRANT_OWNER_ACTOR, Value::Null, tag, None)
                .await;
            assert_eq!(status, 409);
            assert_eq!(challenge["confirmation"]["exposes_to_public_links"], true);
            assert_share_missing(&f, &current_download).await;
            assert_eq!(
                f.share_request(
                    "POST",
                    &restore,
                    GRANT_OWNER_ACTOR,
                    Value::Null,
                    tag,
                    challenge["confirmation"]["token"].as_str()
                )
                .await
                .0,
                204
            );
            assert_eq!(f.request("GET", &current_download, None, &[]).await.0, 200);
        }
    }
    assert_share_missing(&f, browse).await;
    assert_share_missing(&f, public_path(&added_link)).await;
    assert_share_missing(&f, public_path(&standalone)).await;
    f.close().await;
}

#[tokio::test]
async fn public_folder_authority_expiry_revocation_and_quota_share_the_file_link_lifecycle() {
    use serde_json::json;
    let f = Fixture::new("public-folder-lifecycle").await;
    f.seed_grant_admin().await;
    let expired = issue_folder_link(&f, ROOT, Some(1)).await;
    let disabled = issue_folder_link(&f, ROOT, Some(30)).await;
    let revoked = issue_folder_link(&f, ROOT, None).await;
    assert_eq!(
        disabled["expires_at"].as_i64().unwrap() - disabled["created_at"].as_i64().unwrap(),
        2_592_000
    );
    let expired_id = expired["id"].as_str().unwrap().to_owned();
    f.workers
        .as_ref()
        .unwrap()
        .transact(move |db| {
            db.execute(
                "UPDATE share_link SET created_at=1,expires_at=86401 WHERE share_id=?1",
                [expired_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    revoke_public_link(&f, &revoked).await;
    for link in [&expired, &revoked] {
        assert_share_missing(&f, public_path(link)).await;
        assert_share_missing(
            &f,
            &format!("{}/files/{SHARE_FILE}", folder_link_base(link)),
        )
        .await;
    }
    let active_path = format!("{}/files/{SHARE_FILE}", folder_link_base(&disabled));
    assert_eq!(f.request("GET", &active_path, None, &[]).await.0, 200);
    let (status, list) = f.share_get("/share-links", GRANT_OWNER_ACTOR).await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "Files");
    assert!(list[0].get("url").is_none());
    for _ in 0..49 {
        assert_eq!(issue_share(&f, None).await.0, 201);
    }
    let (status, reply) = f
        .share_request(
            "POST",
            "/share-links",
            GRANT_OWNER_ACTOR,
            json!({"entry_id":ROOT}),
            None,
            None,
        )
        .await;
    assert_eq!(
        (status, reply["code"].as_str()),
        (409, Some("share_link_limit"))
    );
    assert_eq!(issue_share(&f, None).await.0, 409);
    f.change_grant_authority(OWNER, "disable", GRANT_OWNER_ACTOR)
        .await;
    assert_share_missing(&f, public_path(&disabled)).await;
    assert_share_missing(&f, &active_path).await;
    assert_eq!(
        f.share_request(
            "POST",
            "/share-links",
            GRANT_OWNER_ACTOR,
            json!({"entry_id":ROOT}),
            None,
            None
        )
        .await
        .0,
        401
    );
    f.change_grant_authority(OWNER, "enable", GRANT_OWNER_ACTOR)
        .await;
    assert_share_missing(&f, public_path(&disabled)).await;
    assert_share_missing(&f, &active_path).await;
    assert_eq!(
        f.share_get("/share-links", GRANT_OWNER_ACTOR).await.1,
        json!([])
    );
    let new = issue_folder_link(&f, ROOT, None).await;
    assert_eq!(f.request("GET", public_path(&new), None, &[]).await.0, 200);
    f.close().await;
}

#[tokio::test]
async fn authorized_public_folder_download_finishes_after_revoke_and_browsing_uses_no_slot() {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let bytes = vec![b'a'; 16 * 1024 * 1024];
    let f = Fixture::start("public-folder-download-slot", None, ("large.bin", &bytes)).await;
    let (_, file) = issue_share(&f, None).await;
    let folder = issue_folder_link(&f, ROOT, None).await;
    let saved = format!("{}/files/{SHARE_FILE}", folder_link_base(&folder));

    let address = f.server.as_ref().unwrap().address();
    let mut held = Vec::new();
    for n in 0..8 {
        let path = if n % 2 == 0 {
            &saved
        } else {
            public_path(&file)
        };
        let mut stream = tls_connect(address, &f.ca, "127.0.0.1").await.unwrap();
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut body = BufReader::new(stream);
        let mut line = String::new();
        body.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim(), "HTTP/1.1 200 OK");
        loop {
            line.clear();
            assert_ne!(body.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        held.push(body);
    }
    let (status, headers, _) = f.request("GET", &saved, None, &[]).await;
    assert_eq!(status, 429);
    assert_eq!(headers["retry-after"], "1");
    assert_eq!(f.request("GET", public_path(&file), None, &[]).await.0, 429);
    assert_eq!(
        f.request("GET", public_path(&folder), None, &[]).await.0,
        200
    );
    revoke_public_link(&f, &folder).await;
    assert_share_missing(&f, public_path(&folder)).await;
    let mut active = held.remove(0);
    drop(held);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = f.request("GET", &saved, None, &[]).await.0;
            if status == 404 {
                break;
            }
            assert_eq!(status, 429);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut actual = Vec::new();
    active.read_to_end(&mut actual).await.unwrap();
    assert_eq!(actual.len(), bytes.len());
    assert_eq!(Sha256::digest(&actual), Sha256::digest(&bytes));
    assert_share_missing(&f, &saved).await;
    assert_eq!(f.request("GET", public_path(&file), None, &[]).await.0, 200);
    f.close().await;
}

#[tokio::test]
async fn public_folder_pages_keep_the_cursor_inside_the_current_folder() {
    let f = Fixture::new("public-folder-pages").await;
    let root = f.share_folder(ROOT, "Shared root").await;
    let folder_id = root.clone();
    f.workers
        .as_ref()
        .unwrap()
        .transact(move |db| {
            for i in 0..=100 {
                let name = format!("Folder {i:03}");
                db.execute(
                    "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
                     VALUES(?1,?2,?3,'folder',?4,?5,1)",
                    params![format!("{:032x}", i + 1024), OWNER, folder_id, name, name.as_bytes()],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let share = issue_folder_link(&f, &root, None).await;
    let base = folder_link_base(&share);
    let cursor = format!("{:032x}", 99 + 1024);
    let next = format!("{base}/folders/{root}?after={cursor}");
    let (status, _, first) = f.request("GET", public_path(&share), None, &[]).await;
    assert_eq!(status, 200);
    let first = String::from_utf8(first).unwrap();
    assert!(first.contains("Folder 099"));
    assert!(!first.contains("Folder 100"));
    assert!(first.contains(&next));
    let (status, _, last) = f.request("GET", &next, None, &[]).await;
    assert_eq!(status, 200);
    let last = String::from_utf8(last).unwrap();
    assert!(last.contains("Folder 100"));
    assert!(!last.contains("Folder 099"));
    assert!(!last.contains("Next page"));
    assert_share_missing(&f, &format!("{base}/folders/{root}?after={SHARE_FILE}")).await;
    f.close().await;
}
