use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, warn};

use agent_types::lsp::{
    LspCallHierarchyItem, LspDiagnostic, LspError, LspIncomingCall, LspLocation, LspOutgoingCall,
    LspSymbol, Severity,
};

use crate::client::LspClient;
use crate::servers::{path_to_uri, resolve_binary, uri_to_path, ServerConfig};
use crate::types::{symbol_kind_name, LspPos};

const MAX_CRASHES: u32 = 3;
const DIAG_TIMEOUT_SECS: u64 = 30;
const DIAG_SETTLE_MS: u64 = 2500; // quiet-window: return when no new notification for this long
const STARTUP_TIMEOUT_SECS: u64 = 45;

#[derive(Debug, Clone, PartialEq)]
enum State {
    Stopped,
    Running,
    Failed(String),
}

pub struct LspServerInstance {
    config: ServerConfig,
    root: PathBuf,
    state: State,
    client: Option<LspClient>,
    open_files: HashMap<String, i32>, // uri → version
    diagnostics: Arc<Mutex<HashMap<String, Vec<LspDiagnostic>>>>,
    diag_updated: Arc<Notify>,
    crash_count: u32,
    _diag_task: Option<tokio::task::JoinHandle<()>>,
}

impl LspServerInstance {
    pub fn new(config: ServerConfig, root: PathBuf) -> Self {
        Self {
            config,
            root,
            state: State::Stopped,
            client: None,
            open_files: HashMap::new(),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            diag_updated: Arc::new(Notify::new()),
            crash_count: 0,
            _diag_task: None,
        }
    }

    pub async fn ensure_started(&mut self) -> Result<(), LspError> {
        match &self.state {
            State::Running => return Ok(()),
            State::Failed(msg) => return Err(LspError::PermanentlyFailed(msg.clone())),
            State::Stopped => {}
        }

        if self.crash_count >= MAX_CRASHES {
            let msg = format!(
                "server '{}' crashed {} times, giving up",
                self.config.id, self.crash_count
            );
            self.state = State::Failed(msg.clone());
            return Err(LspError::PermanentlyFailed(msg));
        }

        let start = self.start_server().await;
        if let Err(e) = start {
            self.crash_count += 1;
            warn!(
                server = self.config.id,
                crash_count = self.crash_count,
                "LSP server failed to start: {}",
                e
            );
            return Err(e);
        }
        self.state = State::Running;
        debug!(server = self.config.id, root = ?self.root, "LSP server started");
        Ok(())
    }

    async fn start_server(&mut self) -> Result<(), LspError> {
        let root_uri = path_to_uri(&self.root);

        // Resolve binary (checks PATH + global bin dir, auto-installs if configured).
        let binary = resolve_binary(&self.config).await?;
        let binary_str = binary
            .to_str()
            .ok_or_else(|| LspError::StartupFailed("binary path contains invalid UTF-8".into()))?
            .to_string();

        let client = tokio::time::timeout(
            std::time::Duration::from_secs(STARTUP_TIMEOUT_SECS),
            LspClient::start(&binary_str, self.config.args, &self.root),
        )
        .await
        .map_err(|_| LspError::StartupFailed("process spawn timed out".into()))??;

        client
            .initialize(&root_uri, self.config.initialization_options.clone())
            .await
            .map_err(|e| LspError::StartupFailed(e.to_string()))?;

        // Spawn background task to handle publishDiagnostics notifications
        let mut notif_rx = client.subscribe();
        let diag_store = Arc::clone(&self.diagnostics);
        let diag_updated = Arc::clone(&self.diag_updated);
        let task = tokio::spawn(async move {
            while let Ok(notif) = notif_rx.recv().await {
                if notif.method != "textDocument/publishDiagnostics" {
                    continue;
                }
                let uri = match notif.params.get("uri").and_then(|v: &Value| v.as_str()) {
                    Some(u) => u.to_string(),
                    None => continue,
                };
                let raw_diags = match notif.params.get("diagnostics").and_then(|v: &Value| v.as_array()) {
                    Some(d) => d.clone(),
                    None => continue,
                };
                let diags: Vec<LspDiagnostic> = raw_diags.iter().map(parse_diagnostic).collect();
                diag_store.lock().await.insert(uri, diags);
                // notify_one stores one pending wakeup so it's delivered even if
                // no waiter is registered yet (unlike notify_waiters which is lost).
                diag_updated.notify_one();
            }
        });

        self.client = Some(client);
        self._diag_task = Some(task);
        Ok(())
    }

    pub async fn open_file(&mut self, path: &Path, text: String) -> Result<(), LspError> {
        self.ensure_started().await?;
        let uri = path_to_uri(path);

        if let Some(version) = self.open_files.get_mut(&uri) {
            // Already open: send didChange only if content differs from what we last sent.
            // We don't cache the text, so always send didChange to keep the server in sync.
            *version += 1;
            let v = *version;
            self.client().notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": v },
                    "contentChanges": [{ "text": text }]
                }),
            );
        } else {
            self.open_files.insert(uri.clone(), 1);
            self.client().notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": self.config.language_id,
                        "version": 1,
                        "text": text,
                    }
                }),
            );
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn change_file(&mut self, path: &Path, text: String) -> Result<(), LspError> {
        self.ensure_started().await?;
        let uri = path_to_uri(path);
        let version = self.open_files.entry(uri.clone()).or_insert(0);
        *version += 1;
        let version = *version;

        self.client().notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            }),
        );
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn close_file(&mut self, path: &Path) -> Result<(), LspError> {
        if self.state != State::Running {
            return Ok(());
        }
        let uri = path_to_uri(path);
        self.open_files.remove(&uri);
        self.client().notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
        Ok(())
    }

    /// Returns cached diagnostics, waiting up to DIAG_TIMEOUT_SECS for results to stabilise.
    ///
    /// Language servers often send an initial empty publishDiagnostics before analysis
    /// completes.  We handle this with a two-phase wait:
    ///
    /// Phase 1 – wait for any notification for this file.
    ///   • Non-empty ⇒ return immediately.
    ///   • Empty ⇒ fall through to Phase 2.
    ///
    /// Phase 2 – settle window.
    ///   • Each new notification resets the DIAG_SETTLE_MS timer.
    ///   • Non-empty notification ⇒ return immediately.
    ///   • Timer expires (quiet for DIAG_SETTLE_MS) ⇒ return whatever we have.
    pub async fn diagnostics(&self, path: &Path) -> Result<Vec<LspDiagnostic>, LspError> {
        let uri = path_to_uri(path);

        // Fast path: already have non-empty cached diagnostics
        {
            let store = self.diagnostics.lock().await;
            if let Some(diags) = store.get(&uri) {
                if !diags.is_empty() {
                    return Ok(diags.clone());
                }
            }
        }

        let timeout_dur = tokio::time::Duration::from_secs(DIAG_TIMEOUT_SECS);
        let settle = tokio::time::Duration::from_millis(DIAG_SETTLE_MS);

        let diag_store = Arc::clone(&self.diagnostics);
        let diag_updated = Arc::clone(&self.diag_updated);
        let uri_clone = uri.clone();

        let result = tokio::time::timeout(timeout_dur, async move {
            // Phase 1: wait for first notification for our file
            loop {
                diag_updated.notified().await;
                let store = diag_store.lock().await;
                if let Some(diags) = store.get(&uri_clone) {
                    if !diags.is_empty() {
                        return diags.clone(); // errors found — done
                    }
                    break; // got an entry (empty) — enter settle phase
                }
                // notification was for a different file — keep waiting
            }

            // Phase 2: settle — return on quiet, or immediately on non-empty
            loop {
                match tokio::time::timeout(settle, diag_updated.notified()).await {
                    Ok(_) => {
                        let store = diag_store.lock().await;
                        if let Some(diags) = store.get(&uri_clone) {
                            if !diags.is_empty() {
                                return diags.clone();
                            }
                        }
                        // Still empty — reset settle timer (continue loop)
                    }
                    Err(_) => {
                        // Quiet for DIAG_SETTLE_MS — analysis is stable
                        let store = diag_store.lock().await;
                        return store.get(&uri_clone).cloned().unwrap_or_default();
                    }
                }
            }
        })
        .await
        .unwrap_or_default();

        Ok(result)
    }

    /// Wait until the LSP server has sent at least one publishDiagnostics for any file
    /// (used for workspace-scope queries like workspace/symbol).
    async fn wait_for_any_file_indexed(&self) {
        let diag_store = Arc::clone(&self.diagnostics);
        let diag_updated = Arc::clone(&self.diag_updated);
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(DIAG_TIMEOUT_SECS),
            async move {
                loop {
                    diag_updated.notified().await;
                    if !diag_store.lock().await.is_empty() {
                        return;
                    }
                }
            },
        )
        .await;
    }

    /// Wait until the LSP server has sent at least one publishDiagnostics notification
    /// for `path` (even if empty). This signals that the server has parsed and indexed
    /// the file, so hover/definition/call-hierarchy queries will return real results.
    async fn wait_for_file_ready(&self, path: &Path) {
        let uri = path_to_uri(path);
        let diag_store = Arc::clone(&self.diagnostics);
        let diag_updated = Arc::clone(&self.diag_updated);
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(DIAG_TIMEOUT_SECS),
            async move {
                loop {
                    diag_updated.notified().await;
                    if diag_store.lock().await.contains_key(&uri) {
                        return; // server has processed this file (entry exists, even if empty)
                    }
                }
            },
        )
        .await;
    }

    pub async fn hover(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<String>, LspError> {
        self.wait_for_file_ready(path).await;
        let uri = path_to_uri(path);
        let pos = LspPos::from_1based(line, col);
        let result = self
            .client()
            .request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character }
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(None);
        }
        Ok(extract_hover_text(&result))
    }

    pub async fn definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        self.wait_for_file_ready(path).await;
        let uri = path_to_uri(path);
        let pos = LspPos::from_1based(line, col);
        let result = self
            .client()
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character }
                }),
            )
            .await?;

        Ok(parse_locations(&result))
    }

    pub async fn references(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        include_declaration: bool,
    ) -> Result<Vec<LspLocation>, LspError> {
        self.wait_for_file_ready(path).await;
        let uri = path_to_uri(path);
        let pos = LspPos::from_1based(line, col);
        let result = self
            .client()
            .request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character },
                    "context": { "includeDeclaration": include_declaration }
                }),
            )
            .await?;

        Ok(parse_locations(&result))
    }

    pub async fn document_symbols(&self, path: &Path) -> Result<Vec<LspSymbol>, LspError> {
        self.wait_for_file_ready(path).await;
        let uri = path_to_uri(path);
        let result = self
            .client()
            .request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await?;

        Ok(parse_symbols(&result, &uri))
    }

    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<LspSymbol>, LspError> {
        self.wait_for_any_file_indexed().await;
        let result = self
            .client()
            .request("workspace/symbol", json!({ "query": query }))
            .await?;

        Ok(parse_workspace_symbols(&result))
    }

    pub async fn implementation(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        self.wait_for_file_ready(path).await;
        let uri = path_to_uri(path);
        let pos = LspPos::from_1based(line, col);
        let result = self
            .client()
            .request(
                "textDocument/implementation",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character }
                }),
            )
            .await?;
        Ok(parse_locations(&result))
    }

    pub async fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspCallHierarchyItem>, LspError> {
        let raw = self.call_hierarchy_items_raw(path, line, col).await?;
        Ok(raw.iter().filter_map(parse_call_hierarchy_item).collect())
    }

    pub async fn incoming_calls(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspIncomingCall>, LspError> {
        let items = self.call_hierarchy_items_raw(path, line, col).await?;
        let Some(first) = items.into_iter().next() else {
            return Ok(vec![]);
        };
        let result = self
            .client()
            .request("callHierarchy/incomingCalls", json!({ "item": first }))
            .await?;
        Ok(parse_incoming_calls(&result))
    }

    pub async fn outgoing_calls(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspOutgoingCall>, LspError> {
        let uri = path_to_uri(path);
        let items = self.call_hierarchy_items_raw(path, line, col).await?;
        let Some(first) = items.into_iter().next() else {
            return Ok(vec![]);
        };
        let result = self
            .client()
            .request("callHierarchy/outgoingCalls", json!({ "item": first }))
            .await?;
        Ok(parse_outgoing_calls(&result, &uri))
    }

    async fn call_hierarchy_items_raw(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<Value>, LspError> {
        self.wait_for_file_ready(path).await;
        let uri = path_to_uri(path);
        let pos = LspPos::from_1based(line, col);
        let result = self
            .client()
            .request(
                "textDocument/prepareCallHierarchy",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character }
                }),
            )
            .await?;
        Ok(result.as_array().cloned().unwrap_or_default())
    }

    fn client(&self) -> &LspClient {
        self.client.as_ref().expect("ensure_started was called")
    }
}

// ── Response parsing helpers ──────────────────────────────────────────────────

fn parse_diagnostic(v: &Value) -> LspDiagnostic {
    let severity = v
        .get("severity")
        .and_then(|s| s.as_u64())
        .map(|n| Severity::from_lsp_code(n as u32))
        .unwrap_or(Severity::Error);

    let (line, col) = v
        .get("range")
        .and_then(|r| r.get("start"))
        .map(|s| {
            let line = s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            let col = s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            (line, col)
        })
        .unwrap_or((1, 1));

    let message = v
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    let source = v
        .get("source")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    let code = v.get("code").map(|c| match c {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    });

    LspDiagnostic {
        severity: severity.as_str().to_string(),
        line,
        col,
        message,
        source,
        code,
    }
}

fn extract_hover_text(v: &Value) -> Option<String> {
    let contents = v.get("contents")?;
    // MarkedString | MarkupContent | array
    if let Some(s) = contents.as_str() {
        return Some(s.to_string());
    }
    if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
        return Some(value.to_string());
    }
    if let Some(arr) = contents.as_array() {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .or_else(|| item.get("value").and_then(|v| v.as_str()))
            })
            .collect();
        if !parts.is_empty() {
            return Some(parts.join("\n\n"));
        }
    }
    None
}

fn parse_locations(v: &Value) -> Vec<LspLocation> {
    let items = match v {
        Value::Array(arr) => arr.clone(),
        single if single.is_object() => vec![single.clone()],
        _ => return vec![],
    };

    items
        .iter()
        .filter_map(|item| {
            let uri = item
                .get("uri")
                .or_else(|| item.get("targetUri"))
                .and_then(|u| u.as_str())?;
            let range = item
                .get("range")
                .or_else(|| item.get("targetSelectionRange"))
                .or_else(|| item.get("targetRange"))?;
            let start = range.get("start")?;
            let line = start.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            let col =
                start.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            Some(LspLocation {
                file: uri_to_path(uri),
                line,
                col,
            })
        })
        .collect()
}

fn parse_symbols(v: &Value, default_uri: &str) -> Vec<LspSymbol> {
    let items = match v.as_array() {
        Some(a) => a,
        None => return vec![],
    };

    let mut result = Vec::new();
    for item in items {
        collect_symbol(item, None, default_uri, &mut result);
    }
    result
}

fn collect_symbol(
    item: &Value,
    container: Option<&str>,
    default_uri: &str,
    out: &mut Vec<LspSymbol>,
) {
    let name = match item.get("name").and_then(|n| n.as_str()) {
        Some(n) => n.to_string(),
        None => return,
    };
    let kind = item
        .get("kind")
        .and_then(|k| k.as_u64())
        .map(|k| symbol_kind_name(k as u32).to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // DocumentSymbol has "selectionRange", SymbolInformation has "location"
    let location = if let Some(loc) = item.get("location") {
        let uri = loc
            .get("uri")
            .and_then(|u| u.as_str())
            .unwrap_or(default_uri);
        let range = loc.get("range");
        make_location(uri, range)
    } else {
        let range = item.get("selectionRange").or_else(|| item.get("range"));
        make_location(default_uri, range)
    };

    out.push(LspSymbol {
        name: name.clone(),
        kind,
        location,
        container: container.map(|s| s.to_string()),
    });

    // Recurse into children (DocumentSymbol hierarchy)
    if let Some(children) = item.get("children").and_then(|c| c.as_array()) {
        for child in children {
            collect_symbol(child, Some(&name), default_uri, out);
        }
    }
}

fn make_location(uri: &str, range: Option<&Value>) -> LspLocation {
    let (line, col) = range
        .and_then(|r| r.get("start"))
        .map(|s| {
            let line = s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            let col = s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            (line, col)
        })
        .unwrap_or((1, 1));
    LspLocation {
        file: uri_to_path(uri),
        line,
        col,
    }
}

fn parse_call_hierarchy_item(v: &Value) -> Option<LspCallHierarchyItem> {
    let name = v.get("name")?.as_str()?.to_string();
    let kind = v
        .get("kind")
        .and_then(|k| k.as_u64())
        .map(|k| symbol_kind_name(k as u32).to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let uri = v.get("uri")?.as_str()?;
    let range = v.get("selectionRange").or_else(|| v.get("range"))?;
    let start = range.get("start")?;
    let line = start.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
    let col = start.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
    Some(LspCallHierarchyItem {
        name,
        kind,
        file: uri_to_path(uri),
        line,
        col,
    })
}

fn parse_incoming_calls(v: &Value) -> Vec<LspIncomingCall> {
    let items = match v.as_array() {
        Some(a) => a,
        None => return vec![],
    };
    items
        .iter()
        .filter_map(|item| {
            let from = item.get("from")?;
            let caller = parse_call_hierarchy_item(from)?;
            let caller_uri = from.get("uri")?.as_str()?;
            let from_ranges = item
                .get("fromRanges")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            let call_sites = from_ranges
                .iter()
                .filter_map(|range| {
                    let start = range.get("start")?;
                    let line =
                        start.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
                    let col =
                        start.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
                    Some(LspLocation { file: uri_to_path(caller_uri), line, col })
                })
                .collect();
            Some(LspIncomingCall { caller, call_sites })
        })
        .collect()
}

// `fromRanges` in outgoing calls are ranges inside the *queried* file (the caller).
fn parse_outgoing_calls(v: &Value, caller_uri: &str) -> Vec<LspOutgoingCall> {
    let items = match v.as_array() {
        Some(a) => a,
        None => return vec![],
    };
    items
        .iter()
        .filter_map(|item| {
            let to = item.get("to")?;
            let callee = parse_call_hierarchy_item(to)?;
            let from_ranges = item
                .get("fromRanges")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            let call_sites = from_ranges
                .iter()
                .filter_map(|range| {
                    let start = range.get("start")?;
                    let line =
                        start.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
                    let col =
                        start.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
                    Some(LspLocation { file: uri_to_path(caller_uri), line, col })
                })
                .collect();
            Some(LspOutgoingCall { callee, call_sites })
        })
        .collect()
}

fn parse_workspace_symbols(v: &Value) -> Vec<LspSymbol> {
    let items = match v.as_array() {
        Some(a) => a,
        None => return vec![],
    };

    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let kind = item
                .get("kind")
                .and_then(|k| k.as_u64())
                .map(|k| symbol_kind_name(k as u32).to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let loc = item.get("location")?;
            let uri = loc.get("uri")?.as_str()?;
            let range = loc.get("range");
            let location = make_location(uri, range);
            let container = item
                .get("containerName")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
            Some(LspSymbol { name, kind, location, container })
        })
        .collect()
}
