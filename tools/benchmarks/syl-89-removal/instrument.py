#!/usr/bin/env python3
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])

def edit(path, fn):
    p = root / path
    p.write_text(fn(p.read_text()))

def once(text, old, new):
    assert text.count(old) == 1, old
    return text.replace(old, new)

def fingerprint(text):
    text = once(text,
        "    let (fingerprint, count) =\n        fingerprint(tx, owner, &confirmation.selection).map_err(|e| match e {",
        "    let syl89_start = std::time::Instant::now();\n"
        "    let (fingerprint, count) =\n        fingerprint(tx, owner, &confirmation.selection).map_err(|e| match e {")
    text = once(text, "    if fingerprint != confirmation.fingerprint {",
        '    eprintln!("syl89 fingerprint_ns={} roots={count}", syl89_start.elapsed().as_nanos());\n'
        "    if fingerprint != confirmation.fingerprint {")
    start = text.index("    let mut digest = Sha256::new();", text.index("fn fingerprint("))
    end = text.index("\nfn roots(", start)
    body = text[start:end]
    body = "    let profile = std::env::var_os(\"SYL89_PREPARE_BREAKDOWN\").is_some();\n    let mut prepare_ns = 0;\n" + body
    body, edits = re.subn(r'(?m)^(            let mut stmt = db\.prepare\(".*?"\)\?;)$'.replace('.*?', '[\\s\\S]*?'),
        r'            let started = profile.then(std::time::Instant::now);\n\1\n'
        r'            if let Some(started) = started { prepare_ns += started.elapsed().as_nanos(); }', body)
    assert edits == 3, edits
    body = once(body, "    Ok((URL_SAFE_NO_PAD.encode(digest.finalize()), count))",
        '    if profile { eprintln!("syl89 fingerprint_prepare_ns={prepare_ns} roots={count}"); }\n'
        "    Ok((URL_SAFE_NO_PAD.encode(digest.finalize()), count))")
    text = text[:start] + body + text[end:]
    return once(text, "    let (fingerprint, count) = fingerprint(db, owner, &selection)?;",
        "    let started = std::time::Instant::now();\n"
        "    let (fingerprint, count) = fingerprint(db, owner, &selection)?;\n"
        '    eprintln!("syl89 preview_fingerprint_ns={}", started.elapsed().as_nanos());')

edit("src/domain/removal.rs", fingerprint)
edit("src/http/removals.rs", lambda t: once(t,
    "    let result = s\n        .workers\n        .transact_result",
    "    let syl89_writer_start = std::time::Instant::now();\n"
    "    let result = s\n        .workers\n        .transact_result").replace(
    "        .await;\n    match result {\n        Ok(Ok(value)) => {",
    '        .await;\n    eprintln!("syl89 writer_ns={}", syl89_writer_start.elapsed().as_nanos());\n'
    "    match result {\n        Ok(Ok(value)) => {"))
edit("src/persistence/reconcile.rs", lambda t: once(once(t,
    "    let removal_metadata_valid = workers",
    "    let syl89_audit_start = std::time::Instant::now();\n    let removal_metadata_valid = workers"),
    "    if !removal_metadata_valid ||",
    '    eprintln!("syl89 audit_ns={} valid={removal_metadata_valid}", syl89_audit_start.elapsed().as_nanos());\n'
    "    if !removal_metadata_valid ||"))
edit("src/persistence/cleanup.rs", lambda t: once(once(once(once(t,
    "    let candidates = workers.read(move |db| {",
    "    let (candidates, reader_ns) = workers.read(move |db| {\n        let started = std::time::Instant::now();"),
    "        Ok(blobs)", "        Ok((blobs, started.elapsed().as_nanos()))"),
    "    let more = candidates.len() == 32;",
    '    let more = candidates.len() == 32;'),
    "                match batch(&workers, &store, &mut after).await {",
    "                let started = std::time::Instant::now();\n"
    "                match batch(&workers, &store, &mut after).await {").replace(
    "    let cursor = after.clone();",
    "    let syl89_start = std::time::Instant::now();\n    let cursor = after.clone();").replace(
    "    let more = candidates.len() == 32;",
    '    eprintln!("syl89 cleanup_lookup_ns={} reader_ns={reader_ns} candidates={}", syl89_start.elapsed().as_nanos(), candidates.len());\n'
    "    let more = candidates.len() == 32;").replace(
    "                    Ok(more) => {",
    '                    Ok(more) => {\n                        eprintln!("syl89 cleanup_pass_ns={}", started.elapsed().as_nanos());'))
edit("src/main.rs", lambda t: once(once(t,
    "    let recovery = reconcile::reconcile(&workers, &configuration.root).await;",
    "    let syl89_recovery_start = std::time::Instant::now();\n"
    "    let recovery = reconcile::reconcile(&workers, &configuration.root).await;\n"
    '    eprintln!("syl89 reconcile_ns={}", syl89_recovery_start.elapsed().as_nanos());'),
    '    contract_event("listener.bind");',
    '    eprintln!("syl89 ready_addr={}", listening.unwrap().unwrap());\n    contract_event("listener.bind");'))
