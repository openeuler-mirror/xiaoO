use super::*;
use std::{collections::BTreeSet, fs, path::PathBuf};
use tokio::task::JoinHandle;

const ALL_TOOLS: &[&str] = &[
    "ask_user_question",
    "bash",
    "file_edit",
    "file_read",
    "file_write",
    "glob",
    "grep",
    "todo_write",
];
const IN_MEMORY_TOOLS: &[&str] = &["ask_user_question", "todo_write"];

async fn scripted_model(
    expected_tools: &'static [&str],
    steps: Vec<(&'static str, Value, Option<&'static str>)>,
    final_text: bool,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move {
        for (index, (name, args, prior_result)) in steps.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let names: BTreeSet<_> = request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap())
                .collect();
            assert_eq!(names, expected_tools.iter().copied().collect(), "{request}");
            if let Some(expected) = prior_result {
                let messages = request["messages"].as_array().unwrap();
                let tool = messages.iter().rev().find(|m| m["role"] == "tool").unwrap();
                assert!(tool.to_string().contains(expected), "{tool}");
            }
            let chunk = json!({
                "id": "test", "object": "chat.completion.chunk", "created": 0, "model": "scripted",
                "choices": [{"index": 0, "finish_reason": "tool_calls", "delta": {
                    "content": "Preparing client call. ",
                    "tool_calls": [{"index": 0, "id": format!("call-{index}"), "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]
                }}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13}
            });
            let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ).as_bytes()).await.unwrap();
        }
        if final_text {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            reply(
                &mut socket,
                "scripted",
                "done",
                Some(json!({
                    "prompt_tokens": 20, "completion_tokens": 4, "total_tokens": 24
                })),
            )
            .await;
        }
    });
    (endpoint, task)
}

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    host: PathBuf,
    client: PathBuf,
    agent: Agent,
    session: Value,
}

impl Fixture {
    async fn start(endpoint: &str, permission: &str, capabilities: Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let host = root.path().join("workspace");
        let client = root.path().join("client");
        for path in [&home, &host, &client] {
            fs::create_dir(path).unwrap();
        }
        // A valid manifest with an arbitrary name must remain undiscoverable.
        for (name, path) in [
            ("home_extension", &home),
            ("workspace_extension", &host),
            ("client_extension", &client),
        ] {
            let tools = path.join(".xiaoo/tools");
            fs::create_dir_all(&tools).unwrap();
            fs::write(
                tools.join(format!("{name}.toml")),
                format!(
                    r#"
name = "{name}"
description = "Fixture extension"
[input_schema]
type = "object"
[exec]
command = "sh"
args = ["-c", "printf unintended > extension-marker"]
stdin = "none"
"#
                ),
            )
            .unwrap();
        }
        fs::write(host.join("sentinel.txt"), "host sentinel").unwrap();
        let config = root.path().join("acp.toml");
        fs::write(
            &config,
            format!(
                "default_model = \"mock\"\n[delegation]\npermission = \"{permission}\"\nexclusive_writer = true\n{}",
                profile("mock", "scripted", endpoint, "ACP_TEST_KEY")
            ),
        )
        .unwrap();
        let mut agent = spawn_agent(&config, &host, &home);
        let initialized = agent
            .request(
                1,
                "initialize",
                json!({
                    "protocolVersion": 1, "clientCapabilities": capabilities
                }),
            )
            .await;
        assert_eq!(
            initialized["result"]["agentCapabilities"]["_meta"][PROFILE]["mode"],
            "controlled-coding"
        );
        let opened = agent
            .request(
                2,
                "session/new",
                json!({
                    "cwd": host, "mcpServers": []
                }),
            )
            .await;
        let session = opened["result"]["sessionId"].clone();
        assert!(session.is_string(), "{opened}");
        Self {
            _root: root,
            home,
            host,
            client,
            agent,
            session,
        }
    }

    async fn assert_tools(&mut self, names: &[&str]) {
        let list = self
            .agent
            .request(
                3,
                "_org.openeuler.xiaoo/tools/list",
                json!({
                    "sessionId": self.session
                }),
            )
            .await;
        let expected: Vec<_> = names
            .iter()
            .map(|name| {
                json!({
                    "name": name, "source": "builtin"
                })
            })
            .collect();
        assert_eq!(list["result"]["tools"], json!(expected));
    }

    async fn prompt(&mut self, id: u32) {
        self.agent
            .send(
                Some(id),
                "session/prompt",
                json!({
                    "sessionId": self.session,
                    "prompt": [{"type": "text", "text": "Use the client workspace."}]
                }),
            )
            .await;
    }

    fn translate(&self, path: &Value) -> PathBuf {
        let logical = Path::new(path.as_str().unwrap());
        self.client.join(logical.strip_prefix(&self.host).unwrap())
    }

    async fn answer(&mut self, message: &Value, result: Value) {
        let message = json!({"jsonrpc": "2.0", "id": message["id"], "result": result});
        self.agent
            .input
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn close(mut self) {
        let response = self
            .agent
            .request(99, "session/close", json!({"sessionId": self.session}))
            .await;
        assert!(response.get("result").is_some(), "{response}");
        drop(self.agent.input);
        assert!(tokio::time::timeout(DEADLINE, self.agent.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
    }
}

fn capabilities() -> Value {
    json!({"terminal": true, "fs": {"readTextFile": true, "writeTextFile": true}})
}

#[tokio::test]
async fn controlled_tools_execute_in_the_client_workspace() {
    let plan_content = format!("Run client tools{}", "\n".repeat(17_000));
    let steps = vec![
        ("file_read", json!({"file_path":"large-link.txt"}), None),
        (
            "file_write",
            json!({"file_path": "nest/remote.txt", "content": "alpha\n"}),
            Some("within 1 MiB"),
        ),
        ("file_read", json!({"file_path": "nest/remote.txt"}), None),
        (
            "file_edit",
            json!({"file_path": "nest/remote.txt", "old_string": "alpha", "new_string": "bravo"}),
            Some("alpha"),
        ),
        ("glob", json!({"pattern": "**/*.txt"}), None),
        (
            "grep",
            json!({"pattern": "bravo", "path": "nest/remote.txt", "output_mode": "content"}),
            Some("nest/remote.txt"),
        ),
        (
            "grep",
            json!({"pattern":"failing-search", "output_mode":"content", "head_limit":1}),
            Some("bravo"),
        ),
        (
            "bash",
            json!({"command": "printf terminal-marker > from-bash.txt"}),
            Some("rg exited with code Some(2)"),
        ),
        (
            "todo_write",
            json!({"todos": [{"content": plan_content, "status": "completed"}]}),
            None,
        ),
    ];
    let count = steps.len();
    let (endpoint, model) = scripted_model(ALL_TOOLS, steps, true).await;
    let mut fixture = Fixture::start(&endpoint, "ask", capabilities()).await;
    fixture.assert_tools(ALL_TOOLS).await;
    let large = fixture.client.join("large.txt");
    fs::File::create(&large)
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();
    std::os::unix::fs::symlink(&large, fixture.client.join("large-link.txt")).unwrap();
    fixture.prompt(4).await;

    let mut terminal = std::collections::HashMap::new();
    let mut next_terminal = 1;
    let mut finished = BTreeSet::new();
    let mut announced = BTreeSet::new();
    let mut failed = 0;
    let mut prefaces = 0;
    let mut approvals = 0;
    let mut plan_completed = false;
    let mut reads = 0;
    let mut writes = 0;
    loop {
        let message = fixture.agent.next().await;
        let params = &message["params"];
        let Some(method) = message["method"].as_str() else {
            assert_eq!(message["id"], 4);
            assert_eq!(message["result"]["stopReason"], "end_turn", "{message}");
            let usage = &message["result"]["_meta"][USAGE];
            assert_eq!(
                usage,
                &json!({
                    "version": 1, "inputTokens": count * 10 + 20,
                    "outputTokens": count * 3 + 4, "status": "complete"
                })
            );
            break;
        };
        assert_eq!(params["sessionId"], fixture.session);
        let result = match method {
            "session/update" => {
                let update = &params["update"];
                if update["sessionUpdate"] == "agent_message_chunk"
                    && update["content"]["text"] == "Preparing client call. "
                {
                    prefaces += 1;
                } else if update["sessionUpdate"] == "tool_call" {
                    assert_eq!(
                        prefaces,
                        announced.len() + 1,
                        "text must arrive before the tool"
                    );
                    assert!(announced.insert(update["toolCallId"].as_str().unwrap().to_owned()));
                } else if update["sessionUpdate"] == "tool_call_update"
                    && matches!(update["status"].as_str(), Some("completed" | "failed"))
                {
                    failed += usize::from(update["status"] == "failed");
                    assert!(finished.insert(update["toolCallId"].as_str().unwrap().to_owned()));
                } else if update["sessionUpdate"] == "plan" {
                    assert_eq!(update["entries"][0]["content"], plan_content);
                    assert_eq!(update["entries"][0]["status"], "completed");
                    plan_completed = true;
                }
                continue;
            }
            "session/request_permission" => {
                assert!(announced.contains(params["toolCall"]["toolCallId"].as_str().unwrap()));
                approvals += 1;
                json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}})
            }
            "fs/read_text_file" => {
                reads += 1;
                let path = fixture.translate(&params["path"]);
                assert_ne!(path.file_name().unwrap(), "large-link.txt");
                json!({"content": fs::read_to_string(path).unwrap()})
            }
            "fs/write_text_file" => {
                writes += 1;
                fs::write(
                    fixture.translate(&params["path"]),
                    params["content"].as_str().unwrap(),
                )
                .unwrap();
                json!({})
            }
            "terminal/create" => {
                let command = params["command"].as_str().unwrap();
                let mut args: Vec<_> = params["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| {
                        let text = arg.as_str().unwrap();
                        if Path::new(text).starts_with(&fixture.host) {
                            fixture.translate(arg).to_str().unwrap().to_owned()
                        } else {
                            text.to_owned()
                        }
                    })
                    .collect();
                assert!(params["env"].as_array().is_none_or(Vec::is_empty));
                let (code, output) = if args.iter().any(|arg| arg == "failing-search") {
                    (2, "partial match\n".repeat(8))
                } else {
                    let process = Command::new(command)
                        .args(args.drain(..))
                        .current_dir(fixture.translate(&params["cwd"]))
                        .env_remove("ACP_TEST_KEY")
                        .output()
                        .await
                        .unwrap();
                    let mut output = process.stdout;
                    output.extend(process.stderr);
                    (
                        process.status.code().unwrap(),
                        String::from_utf8(output).unwrap(),
                    )
                };
                let id = format!("terminal-{next_terminal}");
                next_terminal += 1;
                assert!(output.len() as u64 <= params["outputByteLimit"].as_u64().unwrap());
                terminal.insert(id.clone(), (code, output));
                json!({"terminalId": id})
            }
            "terminal/wait_for_exit" => {
                let (code, _) = &terminal[params["terminalId"].as_str().unwrap()];
                json!({"exitCode": code})
            }
            "terminal/output" => {
                let (_, output) = &terminal[params["terminalId"].as_str().unwrap()];
                json!({"output": output, "truncated": false})
            }
            "terminal/release" => {
                terminal
                    .remove(params["terminalId"].as_str().unwrap())
                    .unwrap();
                json!({})
            }
            _ => panic!("unexpected callback: {message}"),
        };
        fixture.answer(&message, result).await;
    }

    assert_eq!(announced.len(), count);
    assert_eq!(finished, announced);
    assert_eq!(failed, 2);
    assert_eq!(approvals, count - 1);
    assert!(plan_completed);
    assert!(reads >= 2 && writes == 2 && terminal.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.client.join("nest/remote.txt")).unwrap(),
        "bravo\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.client.join("from-bash.txt")).unwrap(),
        "terminal-marker"
    );
    assert_eq!(
        fs::read_to_string(fixture.host.join("sentinel.txt")).unwrap(),
        "host sentinel"
    );
    assert!(!fixture.host.join("nest").exists() && !fixture.host.join("from-bash.txt").exists());
    assert!(!fixture.host.join("extension-marker").exists());
    assert_eq!(fs::read_dir(&fixture.home).unwrap().count(), 1);
    model.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn capabilities_and_user_denial_fail_closed_before_callbacks() {
    let terminal_tools: &[&str] = &["ask_user_question", "bash", "glob", "grep", "todo_write"];
    for (caps, names, attempted, arguments) in [
        (json!({}), IN_MEMORY_TOOLS, "bash", json!({"command":"pwd"})),
        (
            json!({"fs":{"readTextFile":true}}),
            IN_MEMORY_TOOLS,
            "file_read",
            json!({"file_path":"sentinel.txt"}),
        ),
        (
            json!({"terminal":true,"fs":{"writeTextFile":true}}),
            terminal_tools,
            "file_write",
            json!({"file_path":"sentinel.txt","content":"denied"}),
        ),
        (capabilities(), ALL_TOOLS, "bash", json!({"command":"pwd"})),
    ] {
        let (endpoint, model) = scripted_model(
            names,
            vec![
                (attempted, arguments, None),
                (
                    "ask_user_question",
                    json!({"questions": [{"kind": "confirm", "prompt": "Proceed?"}]}),
                    Some("tool"),
                ),
            ],
            false,
        )
        .await;
        let mut fixture = Fixture::start(&endpoint, "ask", caps).await;
        fixture.assert_tools(names).await;
        fixture.prompt(4).await;
        let mut denials = 0;
        loop {
            let message = fixture.agent.next().await;
            match message["method"].as_str() {
                Some("session/update") => (),
                Some("session/request_permission") if names == ALL_TOOLS => {
                    denials += 1;
                    fixture
                        .answer(
                            &message,
                            json!({
                                "outcome": {"outcome": "selected", "optionId": "reject_once"}
                            }),
                        )
                        .await;
                }
                Some(_) => panic!("operation escaped the admission gate: {message}"),
                None => {
                    assert_eq!(message["id"], 4);
                    assert_eq!(
                        message["error"]["message"], "interaction_required",
                        "{message}"
                    );
                    break;
                }
            }
        }
        assert_eq!(denials, usize::from(names == ALL_TOOLS));
        model.await.unwrap();
        fixture.close().await;
    }
}

#[tokio::test]
async fn terminal_cancel_kills_and_releases_before_prompt_completion() {
    for (stage, confirm_release) in [
        ("terminal/wait_for_exit", true),
        ("terminal/wait_for_exit", false),
        ("terminal/output", true),
    ] {
        cancel_terminal(stage, confirm_release).await;
    }
}

async fn cancel_terminal(stage: &str, confirm_release: bool) {
    let (endpoint, model) = scripted_model(
        ALL_TOOLS,
        vec![("bash", json!({"command": "sleep 30"}), None)],
        false,
    )
    .await;
    let mut fixture = Fixture::start(&endpoint, "preauthorized", capabilities()).await;
    fixture.prompt(4).await;
    let mut ordered = Vec::new();
    let mut started = None;
    loop {
        let message = fixture.agent.next().await;
        let result = match message["method"].as_str() {
            Some("session/update" | "$/cancel_request") => continue,
            Some("terminal/create") => json!({"terminalId": "running"}),
            Some("terminal/wait_for_exit") if stage == "terminal/output" => json!({"exitCode":0}),
            Some(method) if method == stage => {
                started = Some(tokio::time::Instant::now());
                fixture
                    .agent
                    .send(
                        None,
                        "session/cancel",
                        json!({"sessionId": fixture.session}),
                    )
                    .await;
                continue;
            }
            Some(method @ ("terminal/kill" | "terminal/release")) => {
                assert_eq!(message["params"]["terminalId"], "running");
                ordered.push(method.to_owned());
                if method == "terminal/release" && !confirm_release {
                    continue;
                }
                json!({})
            }
            None => {
                assert_eq!(message["id"], 4);
                if confirm_release {
                    assert_eq!(message["result"]["stopReason"], "cancelled", "{message}");
                } else {
                    assert_eq!(
                        message["error"]["message"], "operation_result_unknown",
                        "{message}"
                    );
                    let next = fixture.agent.request(5, "session/prompt", json!({
                        "sessionId": fixture.session, "prompt": [{"type": "text", "text": "Continue"}]
                    })).await;
                    assert_eq!(next["error"]["message"], "operation_result_unknown");
                }
                break;
            }
            _ => panic!("unexpected callback: {message}"),
        };
        fixture.answer(&message, result).await;
    }
    assert_eq!(ordered, ["terminal/kill", "terminal/release"]);
    assert!(started.unwrap().elapsed() < Duration::from_secs(5));
    model.await.unwrap();
    fixture.close().await;
}
