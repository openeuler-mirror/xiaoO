//! On-disk request format v1. Public Span reads still return the original JSON.
//! Messages and tools are immutable content-addressed blobs; message sequences
//! are prefix nodes, so appending a message stores one node, not another ID list.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{MoiraiError, Result};

pub(super) fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS request_blobs_v1 (
            hash TEXT PRIMARY KEY, content TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS request_prefixes_v1 (
            hash TEXT PRIMARY KEY, parent_hash TEXT, message_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS span_requests_v1 (
            span_id TEXT PRIMARY KEY, head_hash TEXT,
            metadata_hash TEXT NOT NULL, tools_hash TEXT
        );
        CREATE TRIGGER IF NOT EXISTS delete_span_request_v1 AFTER DELETE ON spans
        BEGIN DELETE FROM span_requests_v1 WHERE span_id = OLD.span_id; END;",
    )?;
    Ok(())
}

// Explicit recursive sorting also works when serde_json's preserve_order feature
// is enabled elsewhere in the workspace. Arrays keep their original order.
fn canonical(mut value: Value) -> Value {
    match &mut value {
        Value::Object(map) => {
            let old = std::mem::take(map);
            let sorted: std::collections::BTreeMap<_, _> = old.into_iter().collect();
            for (key, value) in sorted {
                map.insert(key, canonical(value));
            }
        }
        Value::Array(values) => {
            for value in values {
                *value = canonical(std::mem::take(value));
            }
        }
        _ => {}
    }
    value
}

fn hash(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn put_blob(conn: &Connection, value: Value) -> Result<String> {
    let content = serde_json::to_string(&canonical(value))?;
    let hash = hash(b"moirai/request/blob/v1\0", content.as_bytes());
    conn.prepare_cached(
        "INSERT INTO request_blobs_v1(hash, content) VALUES (?1, ?2)
         ON CONFLICT(hash) DO NOTHING",
    )?
    .execute(params![hash, content])?;
    Ok(hash)
}

/// Must run inside the transaction that writes the owning span. Ordinary extras
/// updates leave the reference untouched, without materializing the request.
pub(super) fn encode(conn: &Connection, span_id: &str, extras: &mut Value) -> Result<()> {
    let Some(request) = extras.get("effective_request") else {
        return Ok(());
    };
    if !request.is_object() || !request.get("messages").is_some_and(Value::is_array) {
        // Preserve arbitrary JSON and the existing shallow replacement semantics.
        conn.execute("DELETE FROM span_requests_v1 WHERE span_id = ?1", [span_id])?;
        return Ok(());
    }
    let mut request = extras
        .as_object_mut()
        .unwrap()
        .remove("effective_request")
        .unwrap();
    let request = request.as_object_mut().unwrap();
    let Value::Array(messages) = request.remove("messages").unwrap() else {
        unreachable!()
    };
    let mut head: Option<String> = None;
    for message in messages {
        let message_hash = put_blob(conn, message)?;
        // JSON tuple encoding gives unambiguous boundaries, including the root.
        let key = serde_json::to_vec(&(&head, &message_hash))?;
        let next = hash(b"moirai/request/prefix/v1\0", &key);
        conn.prepare_cached(
            "INSERT INTO request_prefixes_v1(hash, parent_hash, message_hash)
             VALUES (?1, ?2, ?3) ON CONFLICT(hash) DO NOTHING",
        )?
        .execute(params![next, head, message_hash])?;
        head = Some(next);
    }
    let tools_hash = request
        .remove("tools")
        .map(|tools| put_blob(conn, tools))
        .transpose()?;
    let metadata_hash = put_blob(conn, Value::Object(std::mem::take(request)))?;
    conn.execute(
        "INSERT INTO span_requests_v1(span_id, head_hash, metadata_hash, tools_hash)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(span_id) DO UPDATE SET
         head_hash=excluded.head_hash, metadata_hash=excluded.metadata_hash,
         tools_hash=excluded.tools_hash",
        params![span_id, head, metadata_hash, tools_hash],
    )?;
    Ok(())
}

fn get_blob(conn: &Connection, hash: &str) -> Result<Value> {
    let content: String = conn
        .prepare_cached("SELECT content FROM request_blobs_v1 WHERE hash = ?1")?
        .query_row([hash], |row| row.get(0))?;
    Ok(serde_json::from_str(&content)?)
}

pub(super) fn decode(conn: &Connection, span_id: &str, extras: &mut Value) -> Result<()> {
    let reference = conn
        .prepare_cached(
            "SELECT head_hash, metadata_hash, tools_hash FROM span_requests_v1 WHERE span_id = ?1",
        )?
        .query_row([span_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?;
    let Some((mut head, metadata_hash, tools_hash)) = reference else {
        return Ok(());
    };
    let mut request = get_blob(conn, &metadata_hash)?;
    let mut messages = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Iterative traversal avoids recursion depth limits on long conversations.
    while let Some(hash) = head {
        if !seen.insert(hash.clone()) {
            return Err(MoiraiError::Storage("Cycle in request prefix chain".into()));
        }
        let (parent, message): (Option<String>, String) = conn
            .prepare_cached(
                "SELECT parent_hash, message_hash FROM request_prefixes_v1 WHERE hash = ?1",
            )?
            .query_row([hash], |row| Ok((row.get(0)?, row.get(1)?)))?;
        messages.push(get_blob(conn, &message)?);
        head = parent;
    }
    messages.reverse();
    request["messages"] = Value::Array(messages);
    if let Some(hash) = tools_hash {
        request["tools"] = get_blob(conn, &hash)?;
    }
    extras["effective_request"] = request;
    Ok(())
}

/// Collect only on explicit deletion, never on the hot append/update path.
/// UNION also protects maintenance against cycles in a damaged database.
pub(super) fn collect_unused(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "WITH RECURSIVE live(hash) AS (
            SELECT head_hash FROM span_requests_v1 WHERE head_hash IS NOT NULL
            UNION
            SELECT p.parent_hash FROM request_prefixes_v1 p JOIN live l ON p.hash=l.hash
            WHERE p.parent_hash IS NOT NULL
         ) DELETE FROM request_prefixes_v1 WHERE hash NOT IN (SELECT hash FROM live);
         DELETE FROM request_blobs_v1 WHERE hash NOT IN (
            SELECT message_hash FROM request_prefixes_v1
            UNION SELECT metadata_hash FROM span_requests_v1
            UNION SELECT tools_hash FROM span_requests_v1 WHERE tools_hash IS NOT NULL
         );",
    )?;
    Ok(())
}
