use std::{path::Path, process::Stdio, time::Duration};

use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::oneshot,
};

const DEADLINE: Duration = Duration::from_secs(15);
const PROFILE: &str = "org.openeuler.xiaoo/acp";
const USAGE: &str = "org.openeuler.xiaoo/usage";

struct Agent {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Agent {
    async fn send(&mut self, id: Option<u32>, method: &str, params: Value) {
        let mut message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if let Some(id) = id {
            message["id"] = json!(id);
        }
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
    }

    async fn next(&mut self) -> Value {
        let mut line = String::new();
        let bytes = tokio::time::timeout(DEADLINE, self.output.read_line(&mut line))
            .await
            .expect("ACP message deadline")
            .unwrap();
        assert_ne!(bytes, 0, "unexpected ACP EOF");
        assert!(!line.contains("isolated-test-token"));
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("non-ACP stdout: {line:?}"))
    }

    async fn response(&mut self, id: u32) -> (Value, String) {
        let mut text = String::new();
        loop {
            let message = self.next().await;
            if let Some(method) = message["method"].as_str() {
                assert_eq!(
                    method, "session/update",
                    "unexpected client callback {message}"
                );
                let update = &message["params"]["update"];
                assert!(matches!(
                    update["sessionUpdate"].as_str(),
                    Some("agent_message_chunk" | "agent_thought_chunk")
                ));
                if update["sessionUpdate"] == "agent_message_chunk" {
                    text.push_str(update["content"]["text"].as_str().unwrap());
                }
            } else {
                assert_eq!(message["id"], id);
                return (message, text);
            }
        }
    }

    async fn request(&mut self, id: u32, method: &str, params: Value) -> Value {
        self.send(Some(id), method, params).await;
        self.response(id).await.0
    }
}

async fn receive_request(socket: &mut TcpStream) -> Value {
    let mut reader = BufReader::new(socket);
    let mut size = None;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim().split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                size = Some(value.trim().parse::<usize>().unwrap());
            }
            if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
        }
    }
    assert_eq!(authorization.as_deref(), Some("Bearer isolated-test-token"));
    let mut body = vec![0; size.expect("HTTP request length")];
    reader.read_exact(&mut body).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert!(
        value["tools"].is_null() || value["tools"] == json!([]),
        "{value}"
    );
    value
}

async fn reply(socket: &mut TcpStream, model: &str, text: &str, usage: Option<Value>) {
    let mut chunk = json!({
        "id":"test", "object":"chat.completion.chunk", "created":0, "model":model,
        "choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]
    });
    if let Some(usage) = usage {
        chunk["usage"] = usage;
    }
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    socket.write_all(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    ).as_bytes()).await.unwrap();
}

fn profile(id: &str, model: &str, endpoint: &str, key: &str) -> String {
    format!("[models.{id}]\nprovider = \"openai\"\nmodel = \"{model}\"\napi_base = \"{endpoint}\"\napi_key_env = \"{key}\"\n")
}

fn contents(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn spawn_agent(config: &Path, cwd: &Path, home: &Path) -> Agent {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xiaoo-acp"))
        .arg("--config")
        .arg(config)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("ACP_TEST_KEY", "isolated-test-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    Agent {
        input: child.stdin.take().unwrap(),
        output: BufReader::new(child.stdout.take().unwrap()),
        child,
    }
}

#[tokio::test]
async fn controlled_text_session_over_stdio() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let cwd = root.path().join("process-cwd");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("keep"), b"untouched").unwrap();
    std::fs::write(home.join("keep"), b"untouched").unwrap();
    let home_before = contents(&home);
    let cwd_before = contents(&cwd);
    let provider = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        provider.local_addr().unwrap()
    );
    let config = root.path().join("agent.toml");
    std::fs::write(
        &config,
        format!(
            "default_model = \"first\"\n{}{}{}",
            profile("first", "one", &endpoint, "ACP_TEST_KEY"),
            profile("second", "two", &endpoint, "ACP_TEST_KEY"),
            profile("unavailable", "no-secret", &endpoint, "ACP_MISSING_KEY"),
        ),
    )
    .unwrap();

    let binary = env!("CARGO_BIN_EXE_xiaoo-acp");
    let version = Command::new(binary)
        .arg("--version")
        .output()
        .await
        .unwrap();
    assert!(version.status.success());
    let version = std::str::from_utf8(&version.stdout).unwrap().trim();
    let verbose = Command::new(binary)
        .args(["--version", "--verbose"])
        .output()
        .await
        .unwrap();
    assert!(verbose.status.success());
    let verbose = std::str::from_utf8(&verbose.stdout).unwrap().trim();

    let mut agent = spawn_agent(&config, &cwd, &home);
    let setup = json!({"cwd":"/client-workspace", "mcpServers":[]});
    let pre_init = agent.request(1, "session/new", setup.clone()).await;
    assert_eq!(pre_init["error"]["code"], -32600);
    let handshake = agent
        .request(
            2,
            "initialize",
            json!({
                "protocolVersion": 2,
                "info": {"name":"test-client", "version":"1.0.0"},
                "capabilities": {}
            }),
        )
        .await;
    assert!(handshake.get("error").is_none(), "{handshake}");
    let handshake = &handshake["result"];
    assert_eq!(handshake["protocolVersion"], 1);
    assert_eq!(handshake["agentInfo"]["name"], "xiaoo-acp");
    assert_eq!(handshake["agentInfo"]["version"], version);
    let commit = handshake["_meta"]["org.openeuler.xiaoo/build"]["commit"]
        .as_str()
        .unwrap();
    assert_eq!(verbose, format!("xiaoo-acp {version} (commit {commit})"));
    assert_eq!(
        handshake["agentCapabilities"]["_meta"][PROFILE]["mode"],
        "controlled-text"
    );
    assert_ne!(handshake["agentCapabilities"]["loadSession"], true);
    assert_ne!(
        handshake["agentCapabilities"]["mcpCapabilities"]["http"],
        true
    );
    assert_ne!(
        handshake["agentCapabilities"]["mcpCapabilities"]["sse"],
        true
    );

    let mcp = agent
        .request(
            3,
            "session/new",
            json!({
                "cwd":"/client-workspace",
                "mcpServers":[{"name":"stdio", "command":"/bin/false", "args":[], "env":[]}]
            }),
        )
        .await;
    assert_eq!(
        mcp["error"]["message"],
        "the controlled profile requires empty mcpServers"
    );
    let missing = agent.request(4, "session/new", json!({
        "cwd":"/client-workspace", "mcpServers":[], "_meta":{PROFILE:{"modelProfile":"unavailable"}}
    })).await;
    assert_eq!(missing["error"]["code"], -32000);

    let first = agent.request(5, "session/new", setup.clone()).await;
    let first = first["result"]["sessionId"].as_str().unwrap();
    let second = agent.request(6, "session/new", json!({
        "cwd":"/client-workspace", "mcpServers":[], "_meta":{PROFILE:{"modelProfile":"second"}}
    })).await;
    let second = second["result"]["sessionId"].as_str().unwrap();
    assert_ne!(first, second);
    let (in_flight, wait_for_request) = oneshot::channel();
    let provider = tokio::spawn(async move {
        let (mut socket, _) = provider.accept().await.unwrap();
        let request = receive_request(&mut socket).await;
        assert_eq!(request["model"], "one");
        assert!(request["messages"]
            .to_string()
            .contains("file:///client-workspace/missing"));
        reply(
            &mut socket,
            "one",
            "first reply",
            Some(json!({
                "prompt_tokens":9, "completion_tokens":4, "total_tokens":13
            })),
        )
        .await;
        drop(socket);

        let (mut socket, _) = provider.accept().await.unwrap();
        let request = receive_request(&mut socket).await;
        assert_eq!(request["model"], "two");
        assert!(!request["messages"].to_string().contains("first reply"));
        in_flight.send(()).unwrap();
        let pending = tokio::spawn(async move {
            let mut buf = [0];
            socket.read(&mut buf).await
        });

        let (mut socket, _) = provider.accept().await.unwrap();
        let request = receive_request(&mut socket).await;
        assert_eq!(request["model"], "two");
        assert!(request["messages"].to_string().contains("first reply"));
        reply(&mut socket, "two", "after switch", None).await;
        pending.abort();
    });

    agent.send(Some(7), "session/prompt", json!({"sessionId":first,"prompt":[
        {"type":"text","text":"hello"},
        {"type":"resource_link","name":"workspace reference","uri":"file:///client-workspace/missing"}
    ]})).await;
    let (reply, text) = agent.response(7).await;
    assert_eq!(reply["result"]["stopReason"], "end_turn", "{reply}");
    assert_eq!(text, "first reply");
    assert_eq!(
        reply["result"]["_meta"][USAGE],
        json!({
            "version":1,"inputTokens":9,"outputTokens":4,"status":"complete"
        })
    );

    agent
        .send(
            Some(8),
            "session/prompt",
            json!({"sessionId":second,"prompt":[
                {"type":"text","text":"wait for cancellation"}
            ]}),
        )
        .await;
    tokio::time::timeout(DEADLINE, wait_for_request)
        .await
        .expect("the provider received the prompt before cancellation")
        .unwrap();
    agent
        .send(None, "session/cancel", json!({"sessionId":second}))
        .await;
    let (cancelled, _) = tokio::time::timeout(Duration::from_secs(5), agent.response(8))
        .await
        .expect("cancelled prompt settles within its cleanup deadline");
    assert_eq!(
        cancelled["result"]["stopReason"], "cancelled",
        "{cancelled}"
    );

    let selected = agent
        .request(
            9,
            "session/set_config_option",
            json!({
                "sessionId":first,"configId":"model","value":"second"
            }),
        )
        .await;
    assert_eq!(
        selected["result"]["configOptions"][0]["currentValue"],
        "second"
    );
    agent
        .send(
            Some(10),
            "session/prompt",
            json!({"sessionId":first,"prompt":[
                {"type":"text","text":"continue"}
            ]}),
        )
        .await;
    let (reply, text) = agent.response(10).await;
    assert_eq!(text, "after switch");
    assert_eq!(
        reply["result"]["_meta"][USAGE],
        json!({
            "version":1,"inputTokens":null,"outputTokens":null,"status":"unavailable"
        })
    );
    let failed = agent
        .request(
            11,
            "session/prompt",
            json!({"sessionId":first,"prompt":[
                {"type":"image","data":"","mimeType":"image/png"}
            ]}),
        )
        .await;
    assert_eq!(failed["error"]["code"], -32602);
    assert_eq!(
        agent
            .request(12, "session/close", json!({"sessionId": first}))
            .await["result"],
        json!({})
    );

    drop(agent.input);
    let status = tokio::time::timeout(DEADLINE, agent.child.wait())
        .await
        .unwrap()
        .unwrap();
    let mut stderr = String::new();
    agent
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    assert!(status.success(), "{stderr}");
    assert!(!stderr.contains("isolated-test-token"));
    provider.await.unwrap();
    assert_eq!(contents(&cwd), cwd_before);
    assert_eq!(contents(&home), home_before);
}

async fn start_text_fixture(provider: &TcpListener, root: &Path) -> (Agent, String) {
    let config = root.join("acp.toml");
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        provider.local_addr().unwrap()
    );
    std::fs::write(
        &config,
        format!(
            "default_model = \"test\"\n{}context_window = 4096\n",
            profile("test", "test-model", &endpoint, "ACP_TEST_KEY"),
        ),
    )
    .unwrap();
    let mut agent = spawn_agent(&config, root, root);
    assert_eq!(
        agent
            .request(1, "initialize", json!({"protocolVersion": 1}))
            .await["result"]["protocolVersion"],
        1
    );
    let opened = agent
        .request(2, "session/new", json!({"cwd":"/client","mcpServers":[]}))
        .await;
    let id = opened["result"]["sessionId"]
        .as_str()
        .expect("session ID")
        .to_owned();
    (agent, id)
}

#[tokio::test]
async fn close_cancels_in_flight_compaction_before_releasing_session() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (mut agent, session) = start_text_fixture(&listener, root.path()).await;
    let (compacting, wait_for_compaction) = oneshot::channel();

    let provider = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = receive_request(&mut socket).await;
            assert_eq!(request["stream"], true);
            reply(&mut socket, "test-model", "reply", None).await;
        }

        let (mut socket, _) = listener.accept().await.unwrap();
        let request = receive_request(&mut socket).await;
        assert_ne!(request["stream"], true);
        assert!(request["messages"]
            .to_string()
            .contains("covered_message_count"));
        compacting.send(()).unwrap();
        // The compaction request remains blocked until the Agent drops it.
        let mut buf = [0];
        assert_eq!(socket.read(&mut buf).await.unwrap(), 0);
    });

    for call in 3..6 {
        let reply = agent
            .request(
                call,
                "session/prompt",
                json!({"sessionId":session,"prompt":[{"type":"text","text":"seed history"}]}),
            )
            .await;
        assert_eq!(reply["result"]["stopReason"], "end_turn", "{reply}");
    }

    agent
        .send(
            Some(6),
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":"context token ".repeat(1200)}]}),
        )
        .await;
    tokio::time::timeout(DEADLINE, wait_for_compaction)
        .await
        .unwrap()
        .unwrap();
    agent
        .send(Some(7), "session/close", json!({"sessionId":session}))
        .await;

    tokio::time::timeout(Duration::from_secs(4), async {
        assert_eq!(
            agent.response(6).await.0["result"]["stopReason"],
            "cancelled"
        );
        assert_eq!(agent.response(7).await.0["result"], json!({}));
        provider.await.unwrap();
    })
    .await
    .expect("close must cancel the active compaction before returning success");
}

#[tokio::test]
async fn retry_cannot_succeed_after_restarting_published_text() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (mut agent, session) = start_text_fixture(&listener, root.path()).await;
    let (fail_first, wait_for_publication) = oneshot::channel();
    let prefix = "the first reply has already been published";
    let provider = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        receive_request(&mut socket).await;
        let chunk = json!({
            "id":"first","object":"chat.completion.chunk","created":0,"model":"test-model",
            "choices":[{"index":0,"delta":{"content":prefix}}]
        });
        let body = format!("data: {chunk}\n\n");
        socket.write_all(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n",
            body.len()
        ).as_bytes()).await.unwrap();
        wait_for_publication.await.unwrap();
        // End the first attempt without the HTTP chunk terminator. A retry
        // would otherwise return the shortened prefix as a successful answer.
        drop(socket);
        let (mut retry, _) = listener.accept().await.unwrap();
        receive_request(&mut retry).await;
        reply(&mut retry, "test-model", "the first reply", None).await;
    });

    agent
        .send(
            Some(3),
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":"reply once"}]}),
        )
        .await;
    let update = agent.next().await;
    assert_eq!(update["method"], "session/update");
    assert_eq!(update["params"]["update"]["content"]["text"], prefix);
    fail_first.send(()).unwrap();

    let (response, remaining_text) = agent.response(3).await;
    assert_eq!(
        response["error"]["message"], "prompt output could not be delivered in order",
        "{response}"
    );
    assert!(remaining_text.is_empty());
    provider.abort();
    let _ = provider.await;
}
