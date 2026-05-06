use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{broadcast, oneshot, Mutex};

use agent_types::lsp::LspError;

use crate::host::SpawnedProcess;

#[derive(Clone)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

pub struct LspClient {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    notification_tx: broadcast::Sender<Notification>,
    next_id: Arc<AtomicU64>,
    _child: Arc<Mutex<Child>>,
    _reader_task: tokio::task::JoinHandle<()>,
}

impl LspClient {
    /// Create a client from an already-spawned process (via [`LspEnv::spawn_process`]).
    pub fn from_process(process: SpawnedProcess) -> Self {
        let SpawnedProcess {
            stdin,
            stdout,
            child,
        } = process;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (notification_tx, _) = broadcast::channel(64);

        let reader_task = tokio::spawn(reader_loop(
            stdout,
            Arc::clone(&pending),
            notification_tx.clone(),
        ));

        Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            notification_tx,
            next_id: Arc::new(AtomicU64::new(1)),
            _child: Arc::new(Mutex::new(child)),
            _reader_task: reader_task,
        }
    }

    pub async fn initialize(
        &self,
        root_uri: &str,
        initialization_options: Option<Value>,
    ) -> Result<(), LspError> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "rootPath": root_uri.strip_prefix("file://").unwrap_or(root_uri),
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["plaintext", "markdown"] },
                    "definition": { "linkSupport": false },
                    "references": {},
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "publishDiagnostics": { "relatedInformation": false }
                },
                "workspace": {
                    "symbol": {}
                }
            },
            "initializationOptions": initialization_options,
            "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }]
        });

        self.request("initialize", params).await?;
        self.notify("initialized", json!({}));
        Ok(())
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, LspError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        self.write_message(&msg).await?;

        let resp = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| LspError::Timeout)?
            .map_err(|_| LspError::NotRunning)?;

        if let Some(err) = resp.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(LspError::Rpc { code, message });
        }

        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn notify(&self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let stdin = Arc::clone(&self.stdin);
        let msg_clone = msg.clone();
        tokio::spawn(async move {
            let mut stdin = stdin.lock().await;
            let _ = write_framed(&mut *stdin, &msg_clone).await;
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.notification_tx.subscribe()
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        let _ = self.request("shutdown", Value::Null).await;
        self.notify("exit", Value::Null);
    }

    async fn write_message(&self, msg: &Value) -> Result<(), LspError> {
        let mut stdin = self.stdin.lock().await;
        write_framed(&mut *stdin, msg).await
    }
}

async fn write_framed(stdin: &mut ChildStdin, msg: &Value) -> Result<(), LspError> {
    let body = serde_json::to_string(msg)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(body.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

async fn reader_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    notification_tx: broadcast::Sender<Notification>,
) {
    let mut reader = BufReader::new(stdout);

    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                _ => {}
            }
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(val) = line.strip_prefix("Content-Length:") {
                content_length = val.trim().parse().ok();
            }
        }

        let len = match content_length {
            Some(n) => n,
            None => continue,
        };

        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).await.is_err() {
            return;
        }

        let msg: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
            let mut pending = pending.lock().await;
            if let Some(tx) = pending.remove(&id) {
                let _ = tx.send(msg);
            }
            continue;
        }

        if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let _ = notification_tx.send(Notification {
                method: method.to_string(),
                params,
            });
        }
    }
}
