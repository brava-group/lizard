//! A small ACP v1 client for the Agent sidebar. The agent runs off the UI thread and receives
//! Lizard's existing MCP server, attached to this window's control channel.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use lizard_ui_egui::agent_panel::{AgentEvent, AgentKind, AgentPrompt};
use serde_json::{Value, json};

const MAX_LINE: usize = 8 * 1024 * 1024;

enum Input {
    Message(Value),
    Prompt(AgentPrompt),
    Cancel,
    Permission(Value, Option<String>),
    Shutdown,
    Closed,
}

pub struct Agent {
    input: Sender<Input>,
    events: Receiver<AgentEvent>,
    pub document: u64,
    pub kind: AgentKind,
    pub conversation: u64,
}

impl Agent {
    pub fn start(kind: AgentKind, document: u64, conversation: u64, port: u16, cwd: &str, cli: &str) -> Result<Self, String> {
        Self::start_with_command(kind, document, conversation, port, cwd, cli, std::env::var_os("LIZARD_AGENT_COMMAND").as_deref())
    }

    fn start_with_command(
        kind: AgentKind,
        document: u64,
        conversation: u64,
        port: u16,
        cwd: &str,
        cli: &str,
        override_command: Option<&std::ffi::OsStr>,
    ) -> Result<Self, String> {
        if kind == AgentKind::Codex && override_command.is_none() {
            return start_codex(document, conversation, port, cwd, cli);
        }
        let (input, requests) = mpsc::channel();
        let (output, events) = mpsc::channel();
        let (program, args): (&str, &[&str]) = match kind {
            AgentKind::Codex => ("codex-acp", &[]),
            AgentKind::Claude => ("claude-agent-acp", &[]),
            AgentKind::Gemini => ("gemini", &["--acp"]),
            AgentKind::Kimi => ("kimi", &["acp"]),
        };
        // A single executable override lets protocol tests use a synthetic ACP process. No
        // shell is involved, so its value is never interpreted as shell code.
        let program = override_command.unwrap_or_else(|| std::ffi::OsStr::new(program));
        let args = if override_command.is_some() { &[][..] } else { args };
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Não foi possível iniciar {}: {e}. Instale o adaptador ACP e entre na sua conta.", kind.label()))?;
        let stdin = child.stdin.take().ok_or("O agente não abriu a entrada ACP")?;
        let stdout = child.stdout.take().ok_or("O agente não abriu a saída ACP")?;
        let read_tx = input.clone();
        std::thread::Builder::new().name("lizard-agent-read".into()).spawn(move || read_messages(stdout, read_tx)).map_err(|e| e.to_string())?;
        let mcp = json!({"name":"lizard", "command":cli, "args":["mcp", "--attach", port.to_string()], "env":[]});
        let cwd = cwd.to_string();
        std::thread::Builder::new()
            .name("lizard-agent".into())
            .spawn(move || run(child, stdin, requests, output, cwd, mcp))
            .map_err(|e| e.to_string())?;
        Ok(Self { input, events, document, kind, conversation })
    }

    pub fn send(&self, prompt: AgentPrompt) -> Result<(), String> {
        self.input.send(Input::Prompt(prompt)).map_err(|_| "O agente terminou a sessão".into())
    }

    pub fn stop(&self) {
        let _ = self.input.send(Input::Cancel);
    }

    pub fn permission(&self, id: Value, option: Option<String>) {
        let _ = self.input.send(Input::Permission(id, option));
    }

    pub fn poll(&self) -> Vec<AgentEvent> {
        self.events.try_iter().take(200).collect()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.input.send(Input::Shutdown);
    }
}

fn read_messages(stdout: impl Read, sender: Sender<Input>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut reader).take((MAX_LINE + 1) as u64).read_until(b'\n', &mut line);
        match read {
            Ok(0) | Err(_) => break,
            Ok(_) if line.len() > MAX_LINE => break,
            Ok(_) => {
                if let Ok(message) = serde_json::from_slice::<Value>(&line)
                    && sender.send(Input::Message(message)).is_err()
                {
                    break;
                }
            }
        }
    }
    let _ = sender.send(Input::Closed);
}

fn send(stdin: &mut ChildStdin, id: Option<u64>, method: &str, params: Value) -> Result<(), String> {
    let mut message = json!({"jsonrpc":"2.0", "method":method, "params":params});
    if let Some(id) = id {
        message["id"] = json!(id);
    }
    writeln!(stdin, "{message}").and_then(|_| stdin.flush()).map_err(|e| format!("Falha na conexão com o agente: {e}"))
}

fn run(mut child: Child, mut stdin: ChildStdin, requests: Receiver<Input>, output: Sender<AgentEvent>, cwd: String, mcp: Value) {
    if let Err(e) = send(
        &mut stdin,
        Some(1),
        "initialize",
        json!({"protocolVersion":1, "clientCapabilities":{}, "clientInfo":{"name":"lizard", "title":"Lizard", "version":env!("CARGO_PKG_VERSION")}}),
    ) {
        let _ = output.send(AgentEvent::Error(e));
        let _ = child.kill();
        return;
    }
    let mut session: Option<String> = None;
    let mut queued = std::collections::VecDeque::new();
    let mut prompt_id = 10_u64;
    let mut active: Option<u64> = None;
    let mut image_supported = false;
    let mut load_supported = false;
    let mut resume_supported = false;
    let mut requested_session: Option<String> = None;
    let mut initialized = false;
    let mut session_starting = false;
    while let Ok(event) = requests.recv() {
        let action = match event {
            Input::Prompt(text) => {
                queued.push_back(text);
                Ok(())
            }
            Input::Cancel => session.as_deref().map_or(Ok(()), |sid| send(&mut stdin, None, "session/cancel", json!({"sessionId":sid}))),
            Input::Permission(id, option) => {
                let outcome = option.map_or_else(|| json!({"outcome":"cancelled"}), |option_id| json!({"outcome":"selected", "optionId":option_id}));
                let message = json!({"jsonrpc":"2.0", "id":id, "result":{"outcome":outcome}});
                writeln!(stdin, "{message}").and_then(|_| stdin.flush()).map_err(|e| e.to_string())
            }
            Input::Message(message) => {
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    if method == "session/update" && active.is_some() {
                        if let Some(event) = update(&message) {
                            let _ = output.send(event);
                        }
                    } else if method == "session/request_permission" {
                        let id = message.get("id").cloned().unwrap_or(Value::Null);
                        let p = &message["params"];
                        let title = p["toolCall"]["title"].as_str().unwrap_or("Ação do agente").to_string();
                        let options = p["options"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| Some((v["optionId"].as_str()?.to_string(), v["name"].as_str()?.to_string()))).collect())
                            .unwrap_or_default();
                        let _ = output.send(AgentEvent::Permission { id, title, options });
                    } else if let Some(id) = message.get("id") {
                        let reply = json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Método não disponível no Lizard"}});
                        let _ = writeln!(stdin, "{reply}").and_then(|_| stdin.flush());
                    }
                    Ok(())
                } else if let Some(error) = message.get("error") {
                    let msg = error["message"].as_str().unwrap_or("Erro do agente").to_string();
                    let _ = output.send(AgentEvent::Error(msg));
                    active = None;
                    Ok(())
                } else {
                    match message.get("id").and_then(Value::as_u64) {
                        Some(1) => {
                            if message["result"]["protocolVersion"].as_u64() != Some(1) {
                                Err("O agente não suporta ACP v1".into())
                            } else {
                                image_supported = message["result"]["agentCapabilities"]["promptCapabilities"]["image"].as_bool() == Some(true);
                                load_supported = message["result"]["agentCapabilities"]["loadSession"].as_bool() == Some(true);
                                resume_supported = message["result"]["agentCapabilities"]["sessionCapabilities"]["resume"].is_object();
                                initialized = true;
                                Ok(())
                            }
                        }
                        Some(2) => {
                            if let Some(id) = message["result"]["sessionId"].as_str().map(str::to_string).or_else(|| requested_session.take()) {
                                session = Some(id.clone());
                                let _ = output.send(AgentEvent::Session(id));
                                let models = &message["result"]["models"]["availableModels"];
                                if let Some(items) = models.as_array() {
                                    let models = items
                                        .iter()
                                        .filter_map(|item| Some((item["modelId"].as_str()?.to_string(), item["name"].as_str()?.to_string())))
                                        .take(30)
                                        .collect();
                                    let _ = output.send(AgentEvent::Models(models));
                                }
                                let _ = output.send(AgentEvent::Ready);
                                Ok(())
                            } else {
                                Err("O agente não criou uma sessão ACP".into())
                            }
                        }
                        Some(id) if active == Some(id) => {
                            active = None;
                            let reason = message["result"]["stopReason"].as_str().unwrap_or("end_turn").to_string();
                            let _ = output.send(AgentEvent::Done(reason));
                            Ok(())
                        }
                        _ => Ok(()),
                    }
                }
            }
            Input::Shutdown => break,
            Input::Closed => {
                if active.is_some() || session.is_none() || !queued.is_empty() {
                    let _ = output.send(AgentEvent::Error("O agente encerrou a conexão ACP".into()));
                }
                break;
            }
        };
        if let Err(error) = action {
            let _ = output.send(AgentEvent::Error(error));
            break;
        }
        if initialized
            && !session_starting
            && session.is_none()
            && let Some(prompt) = queued.front()
        {
            let (method, params) = if let Some(id) = prompt.resume.as_deref().filter(|_| resume_supported || load_supported) {
                requested_session = Some(id.to_string());
                (if resume_supported { "session/resume" } else { "session/load" }, json!({"sessionId":id,"cwd":cwd,"mcpServers":[mcp]}))
            } else {
                ("session/new", json!({"cwd":cwd,"mcpServers":[mcp]}))
            };
            if let Err(error) = send(&mut stdin, Some(2), method, params) {
                let _ = output.send(AgentEvent::Error(error));
                break;
            }
            session_starting = true;
        }
        if active.is_none()
            && let Some(sid) = session.as_deref()
            && let Some(prompt) = queued.pop_front()
        {
            if !prompt.images.is_empty() && !image_supported {
                let _ = output.send(AgentEvent::Error("Este agente ACP não aceita imagens no prompt.".into()));
                continue;
            }
            let mut blocks = vec![json!({"type":"text", "text":prompt.text})];
            blocks.extend(prompt.images.into_iter().map(|image| json!({"type":"image", "mimeType":image.mime, "data":image.data})));
            if let Some(model) = prompt.model.as_deref()
                && let Err(error) = send(&mut stdin, Some(9), "session/set_model", json!({"sessionId":sid,"modelId":model}))
            {
                let _ = output.send(AgentEvent::Error(error));
                break;
            }
            prompt_id = prompt_id.saturating_add(1);
            if let Err(error) = send(&mut stdin, Some(prompt_id), "session/prompt", json!({"sessionId":sid, "prompt":blocks})) {
                let _ = output.send(AgentEvent::Error(error));
                break;
            }
            active = Some(prompt_id);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Resolve a locally installed CLI without relying on the GUI process inheriting a shell PATH.
pub fn program(kind: AgentKind) -> Option<PathBuf> {
    let name = match kind {
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude-agent-acp",
        AgentKind::Gemini => "gemini",
        AgentKind::Kimi => "kimi",
    };
    let executable = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut paths: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|dir| dir.join(&executable)).collect();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.extend([
            home.join(".local/bin").join(&executable),
            home.join(".kimi-code/bin").join(&executable),
            home.join(".hermes/node/bin").join(&executable),
        ]);
    }
    paths.into_iter().find(|path| path.is_file())
}

fn start_codex(document: u64, conversation: u64, port: u16, cwd: &str, cli: &str) -> Result<Agent, String> {
    let program = program(AgentKind::Codex).ok_or("Instale o Codex CLI para usar o agente Codex")?;
    start_codex_with_program(program.as_os_str(), document, conversation, port, cwd, cli)
}

fn start_codex_with_program(program: &std::ffi::OsStr, document: u64, conversation: u64, port: u16, cwd: &str, cli: &str) -> Result<Agent, String> {
    let (input, requests) = mpsc::channel();
    let (output, events) = mpsc::channel();
    let mcp_command = format!("mcp_servers.lizard.command={}", serde_json::to_string(cli).map_err(|e| e.to_string())?);
    let mcp_args = format!("mcp_servers.lizard.args=[\"mcp\",\"--attach\",\"{port}\"]");
    let mut child = Command::new(program)
        .args(["app-server", "--stdio", "-c", &mcp_command, "-c", &mcp_args])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Não foi possível iniciar o Codex: {e}"))?;
    let stdin = child.stdin.take().ok_or("O Codex não abriu a entrada do app-server")?;
    let stdout = child.stdout.take().ok_or("O Codex não abriu a saída do app-server")?;
    let read_tx = input.clone();
    std::thread::Builder::new().name("lizard-codex-read".into()).spawn(move || read_messages(stdout, read_tx)).map_err(|e| e.to_string())?;
    let cwd = cwd.to_string();
    std::thread::Builder::new()
        .name("lizard-codex".into())
        .spawn(move || run_codex(child, stdin, requests, output, cwd))
        .map_err(|e| e.to_string())?;
    Ok(Agent { input, events, document, kind: AgentKind::Codex, conversation })
}

fn run_codex(mut child: Child, mut stdin: ChildStdin, requests: Receiver<Input>, output: Sender<AgentEvent>, cwd: String) {
    if let Err(error) =
        send(&mut stdin, Some(1), "initialize", json!({"clientInfo":{"name":"lizard","title":"Lizard","version":env!("CARGO_PKG_VERSION")}}))
    {
        let _ = output.send(AgentEvent::Error(error));
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    let mut initialized = false;
    let mut starting = false;
    let mut session: Option<String> = None;
    let mut active = false;
    let mut queued: std::collections::VecDeque<AgentPrompt> = std::collections::VecDeque::new();
    while let Ok(event) = requests.recv() {
        let result = match event {
            Input::Prompt(prompt) => {
                queued.push_back(prompt);
                Ok(())
            }
            Input::Cancel => {
                if let Some(sid) = session.as_deref() {
                    send(&mut stdin, Some(90), "turn/interrupt", json!({"threadId":sid}))
                } else {
                    Ok(())
                }
            }
            Input::Permission(_, _) => Ok(()),
            Input::Message(message) => (|| -> Result<(), String> {
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    match method {
                        "item/agentMessage/delta" => {
                            if let Some(chunk) = message["params"]["delta"].as_str() {
                                let _ = output.send(AgentEvent::Text(chunk.chars().take(16_384).collect()));
                            }
                        }
                        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                            if let Some(chunk) = message["params"]["delta"].as_str() {
                                let _ = output.send(AgentEvent::Thinking(chunk.chars().take(16_384).collect()));
                            }
                        }
                        "item/started" | "item/completed" => {
                            let item = &message["params"]["item"];
                            let item_type = item["type"].as_str().unwrap_or_default();
                            if matches!(item_type, "mcpToolCall" | "commandExecution" | "fileChange" | "webSearch") {
                                let id = item["id"].as_str().unwrap_or("tool").to_string();
                                let title = match item_type {
                                    "mcpToolCall" => {
                                        format!("{} · {}", item["server"].as_str().unwrap_or("MCP"), item["tool"].as_str().unwrap_or("Ferramenta"))
                                    }
                                    "commandExecution" => item["command"].as_str().unwrap_or("Comando").chars().take(120).collect(),
                                    "fileChange" => "Alteração de arquivo".into(),
                                    _ => "Busca na web".into(),
                                };
                                let status = Some(if method == "item/completed" { "completed" } else { "in_progress" }.into());
                                let input = match item_type {
                                    "mcpToolCall" => item.get("arguments"),
                                    "commandExecution" => item.get("command"),
                                    "fileChange" => item.get("changes"),
                                    _ => item.get("query"),
                                }
                                .map(|value| (!value.is_null()).then(|| value.clone()));
                                let result = item.get("result").or_else(|| item.get("error")).or_else(|| item.get("aggregatedOutput"));
                                let output_value =
                                    if method == "item/completed" { result.map(|value| (!value.is_null()).then(|| value.clone())) } else { None };
                                let _ = output.send(AgentEvent::Tool { id, title: Some(title), status, input, output: output_value, content: None });
                            }
                        }
                        "turn/completed" => {
                            active = false;
                            let status = message["params"]["turn"]["status"].as_str().unwrap_or("failed");
                            if status == "failed" {
                                let detail = message["params"]["turn"]["error"]["message"].as_str().unwrap_or("A execução do Codex falhou");
                                let _ = output.send(AgentEvent::Error(detail.to_string()));
                            } else {
                                let _ = output.send(AgentEvent::Done(if status == "completed" { "end_turn" } else { "cancelled" }.into()));
                            }
                        }
                        "error" if !message["params"]["willRetry"].as_bool().unwrap_or(false) => {
                            let detail = message["params"]["error"]["message"].as_str().unwrap_or("Erro do Codex");
                            let _ = output.send(AgentEvent::Error(detail.to_string()));
                        }
                        _ => {}
                    }
                    if let Some(id) = message.get("id") {
                        let reply = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Aprovação não disponível nesta sessão"}});
                        let _ = writeln!(stdin, "{reply}").and_then(|_| stdin.flush());
                    }
                    Ok(())
                } else if let Some(error) = message.get("error") {
                    let detail = error["message"].as_str().unwrap_or("Erro do app-server");
                    Err(detail.to_string())
                } else {
                    match message.get("id").and_then(Value::as_u64) {
                        Some(1) => {
                            initialized = true;
                            send(&mut stdin, None, "initialized", json!({}))?;
                            send(&mut stdin, Some(3), "model/list", json!({"limit":100}))
                        }
                        Some(2) => {
                            let sid = message["result"]["thread"]["id"].as_str().ok_or("O Codex não retornou uma thread")?.to_string();
                            session = Some(sid.clone());
                            let _ = output.send(AgentEvent::Session(sid));
                            let _ = output.send(AgentEvent::Ready);
                            Ok(())
                        }
                        Some(3) => {
                            let _ = output.send(AgentEvent::Models(parse_models(&message)));
                            Ok(())
                        }
                        _ => Ok(()),
                    }
                }
            })(),
            Input::Shutdown => break,
            Input::Closed => {
                if active || session.is_none() || !queued.is_empty() {
                    let _ = output.send(AgentEvent::Error("O Codex encerrou a conexão".into()));
                }
                break;
            }
        };
        if let Err(error) = result {
            let _ = output.send(AgentEvent::Error(error));
            break;
        }
        if initialized
            && !starting
            && session.is_none()
            && let Some(prompt) = queued.front()
        {
            let params = if let Some(sid) = prompt.resume.as_deref() {
                json!({"threadId":sid,"cwd":cwd,"approvalPolicy":"never","sandbox":"read-only"})
            } else {
                json!({"cwd":cwd,"approvalPolicy":"never","sandbox":"read-only","model":prompt.model,"ephemeral":false})
            };
            let method = if prompt.resume.is_some() { "thread/resume" } else { "thread/start" };
            if let Err(error) = send(&mut stdin, Some(2), method, params) {
                let _ = output.send(AgentEvent::Error(error));
                break;
            }
            starting = true;
        }
        if !active
            && let Some(sid) = session.as_deref()
            && let Some(prompt) = queued.pop_front()
        {
            let mut input = vec![json!({"type":"text","text":prompt.text})];
            input.extend(prompt.images.into_iter().map(|image| json!({"type":"image","url":format!("data:{};base64,{}",image.mime,image.data)})));
            if let Err(error) = send(&mut stdin, Some(10), "turn/start", json!({"threadId":sid,"input":input,"model":prompt.model})) {
                let _ = output.send(AgentEvent::Error(error));
                break;
            }
            active = true;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn parse_models(message: &Value) -> Vec<(String, String)> {
    message["result"]["data"].as_array().map_or_else(Vec::new, |data| {
        data.iter()
            .filter(|item| item["hidden"].as_bool() != Some(true))
            .filter_map(|item| Some((item["model"].as_str()?.to_string(), item["displayName"].as_str()?.to_string())))
            .take(30)
            .collect()
    })
}

pub fn request_codex_models(events: Sender<AgentEvent>) -> Result<(), String> {
    let program = program(AgentKind::Codex).ok_or("Codex CLI não instalado")?;
    std::thread::Builder::new()
        .name("lizard-codex-models".into())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                let mut child = Command::new(program)
                    .args(["app-server", "--stdio"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| error.to_string())?;
                let mut stdin = child.stdin.take().ok_or("O Codex não abriu a entrada de modelos")?;
                let stdout = child.stdout.take().ok_or("O Codex não abriu a saída de modelos")?;
                let mut reader = BufReader::new(stdout);
                let outcome = (|| -> Result<(), String> {
                    send(
                        &mut stdin,
                        Some(1),
                        "initialize",
                        json!({"clientInfo":{"name":"lizard","title":"Lizard","version":env!("CARGO_PKG_VERSION")}}),
                    )?;
                    let mut requested = false;
                    loop {
                        let mut line = Vec::new();
                        let size = (&mut reader).take((MAX_LINE + 1) as u64).read_until(b'\n', &mut line).map_err(|error| error.to_string())?;
                        if size == 0 || size > MAX_LINE {
                            return Err("O Codex encerrou a consulta de modelos".into());
                        }
                        let Ok(message) = serde_json::from_slice::<Value>(&line) else { continue };
                        if message["id"] == 1 && !requested {
                            send(&mut stdin, None, "initialized", json!({}))?;
                            send(&mut stdin, Some(2), "model/list", json!({"limit":100}))?;
                            requested = true;
                        } else if message["id"] == 2 {
                            let _ = events.send(AgentEvent::Models(parse_models(&message)));
                            return Ok(());
                        }
                    }
                })();
                let _ = child.kill();
                let _ = child.wait();
                outcome
            })();
            if let Err(error) = result {
                log::warn!("codex models: {error}");
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Start the Codex CLI's supported ChatGPT OAuth flow and keep its callback server alive until
/// completion. The browser is opened by the UI after receiving `LoginUrl`.
pub fn login_codex(events: Sender<AgentEvent>) -> Result<(), String> {
    let program = program(AgentKind::Codex).ok_or("Instale o Codex CLI antes de conectar sua conta ChatGPT")?;
    login_codex_with_program(program.as_os_str(), events)
}

fn login_codex_with_program(program: &std::ffi::OsStr, events: Sender<AgentEvent>) -> Result<(), String> {
    let program = program.to_os_string();
    std::thread::Builder::new()
        .name("lizard-codex-login".into())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                let mut child = Command::new(program)
                    .args(["app-server", "--stdio"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| error.to_string())?;
                let mut stdin = child.stdin.take().ok_or("O Codex não abriu a entrada de login")?;
                let stdout = child.stdout.take().ok_or("O Codex não abriu a saída de login")?;
                let mut reader = BufReader::new(stdout);
                let outcome = (|| -> Result<(), String> {
                    send(
                        &mut stdin,
                        Some(1),
                        "initialize",
                        json!({"clientInfo":{"name":"lizard","title":"Lizard","version":env!("CARGO_PKG_VERSION")}}),
                    )?;
                    let mut started = false;
                    let mut url_sent = false;
                    loop {
                        let mut line = Vec::new();
                        let size = (&mut reader).take((MAX_LINE + 1) as u64).read_until(b'\n', &mut line).map_err(|error| error.to_string())?;
                        if size == 0 || size > MAX_LINE {
                            return Err("O Codex encerrou o login antes de concluir".into());
                        }
                        let Ok(message) = serde_json::from_slice::<Value>(&line) else { continue };
                        if message["id"] == 1 && !started {
                            send(&mut stdin, None, "initialized", json!({}))?;
                            send(&mut stdin, Some(2), "account/login/start", json!({"type":"chatgpt"}))?;
                            started = true;
                        } else if message["id"] == 2 {
                            if let Some(error) = message["error"]["message"].as_str() {
                                return Err(error.to_string());
                            }
                            let url = message["result"]["authUrl"].as_str().ok_or("O Codex não forneceu a página de login")?;
                            if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1:") {
                                return Err("O Codex forneceu um endereço de login inválido".into());
                            }
                            let _ = events.send(AgentEvent::LoginUrl(url.to_string()));
                            url_sent = true;
                        } else if message["method"] == "account/login/completed" {
                            if message["params"]["success"].as_bool() == Some(true) {
                                let _ = events.send(AgentEvent::LoginComplete);
                                return Ok(());
                            }
                            return Err(message["params"]["error"].as_str().unwrap_or("O login do Codex não foi concluído").to_string());
                        } else if message["method"] == "account/updated" && url_sent && message["params"]["account"].is_object() {
                            let _ = events.send(AgentEvent::LoginComplete);
                            return Ok(());
                        }
                    }
                })();
                let _ = child.kill();
                let _ = child.wait();
                outcome
            })();
            if let Err(error) = result {
                let _ = events.send(AgentEvent::LoginError(error));
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    #[test]
    fn codex_login_opens_browser_url_and_reports_completion() {
        let folder = std::env::temp_dir().join(format!("lizard-codex-login-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let script = folder.join("mock-login");
        std::fs::write(
            &script,
            r#"#!/usr/bin/env python3
import json, sys
def send(value): print(json.dumps(value), flush=True)
for line in sys.stdin:
 msg=json.loads(line)
 if msg.get('method')=='initialize':
  send({'jsonrpc':'2.0','id':msg['id'],'result':{}})
 elif msg.get('method')=='account/login/start':
  assert msg['params']['type']=='chatgpt'
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'type':'chatgpt','authUrl':'https://example.com/login','loginId':'test'}})
  send({'jsonrpc':'2.0','method':'account/login/completed','params':{'success':True}})
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&script, perms).unwrap();
        let (send, events) = mpsc::channel();
        login_codex_with_program(script.as_os_str(), send).unwrap();
        assert!(matches!(events.recv_timeout(Duration::from_secs(4)), Ok(AgentEvent::LoginUrl(url)) if url == "https://example.com/login"));
        assert!(matches!(events.recv_timeout(Duration::from_secs(4)), Ok(AgentEvent::LoginComplete)));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn codex_app_server_streams_and_resumes_with_document_mcp() {
        let folder = std::env::temp_dir().join(format!("lizard-codex-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let script = folder.join("mock-codex");
        std::fs::write(
            &script,
            r#"#!/usr/bin/env python3
import json, sys
assert any('mcp_servers.lizard.command=' in value for value in sys.argv)
assert any('mcp_servers.lizard.args=' in value and '7777' in value for value in sys.argv)
def send(value): print(json.dumps(value), flush=True)
for line in sys.stdin:
 msg=json.loads(line)
 method=msg.get('method')
 if method=='initialize':
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'userAgent':'mock'}})
 elif method=='model/list':
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'data':[{'model':'test-model','displayName':'Test model','hidden':False}]}})
 elif method in ('thread/start','thread/resume'):
  if method=='thread/resume': assert msg['params']['threadId']=='saved-thread'
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'thread':{'id':'saved-thread'}}})
 elif method=='turn/start':
  assert msg['params']['threadId']=='saved-thread'
  assert msg['params']['model']=='test-model'
  assert msg['params']['input'][1]['url']=='data:image/png;base64,aGVsbG8='
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'turn':{'id':'turn-1'}}})
  send({'jsonrpc':'2.0','method':'item/started','params':{'item':{'id':'tool-1','type':'mcpToolCall','server':'lizard','tool':'create_node'}}})
  send({'jsonrpc':'2.0','method':'item/agentMessage/delta','params':{'delta':'Created frame'}})
  send({'jsonrpc':'2.0','method':'item/completed','params':{'item':{'id':'tool-1','type':'mcpToolCall','server':'lizard','tool':'create_node'}}})
  send({'jsonrpc':'2.0','method':'turn/completed','params':{'turn':{'status':'completed'}}})
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&script, perms).unwrap();
        for resume in [None, Some("saved-thread".into())] {
            let agent = start_codex_with_program(script.as_os_str(), 42, 1, 7777, &folder.to_string_lossy(), "lizard-cli").unwrap();
            agent
                .send(AgentPrompt {
                    text: "create frame".into(),
                    images: vec![lizard_ui_egui::agent_panel::AgentImage { mime: "image/png".into(), data: "aGVsbG8=".into() }],
                    conversation: 1,
                    resume,
                    resumes: Vec::new(),
                    model: Some("test-model".into()),
                    agent_count: 1,
                    parallel_mode: lizard_ui_egui::agent_panel::ParallelMode::Split,
                    variant_count: 1,
                    variant_mode: lizard_ui_egui::agent_panel::VariantMode::Style,
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(4);
            let mut got_session = false;
            let mut got_model = false;
            let mut got_text = false;
            let mut got_tool = false;
            let mut got_done = false;
            while Instant::now() < deadline && !got_done {
                for event in agent.poll() {
                    match event {
                        AgentEvent::Session(id) => got_session |= id == "saved-thread",
                        AgentEvent::Models(models) => got_model |= models.first().is_some_and(|(id, _)| id == "test-model"),
                        AgentEvent::Text(text) => got_text |= text == "Created frame",
                        AgentEvent::Tool { status, .. } => got_tool |= status.as_deref() == Some("completed"),
                        AgentEvent::Done(reason) => got_done = reason == "end_turn",
                        AgentEvent::Error(error) => panic!("{error}"),
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(got_session && got_model && got_text && got_tool && got_done);
        }
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn streams_tools_and_permission_through_acp() {
        let folder = std::env::temp_dir().join(format!("lizard-acp-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let script = folder.join("mock-acp");
        std::fs::write(&script, r#"#!/usr/bin/env python3
import json, sys
def send(value):
 print(json.dumps(value), flush=True)
for line in sys.stdin:
 msg=json.loads(line)
 method=msg.get('method')
 if method=='initialize':
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'protocolVersion':1,'agentCapabilities':{'promptCapabilities':{'image':True},'loadSession':True}}})
 elif method=='session/new':
  assert msg['params']['mcpServers'][0]['name']=='lizard'
  assert msg['params']['mcpServers'][0]['args']==['mcp','--attach','7777']
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'sessionId':'session-1'}})
 elif method=='session/load':
  assert msg['params']['sessionId']=='session-1'
  send({'jsonrpc':'2.0','id':msg['id'],'result':{}})
 elif method=='session/prompt':
  assert 'create a frame' in msg['params']['prompt'][0]['text']
  assert msg['params']['prompt'][1]=={'type':'image','mimeType':'image/png','data':'aGVsbG8='}
  send({'jsonrpc':'2.0','method':'session/update','params':{'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'Working'}}}})
  send({'jsonrpc':'2.0','method':'session/update','params':{'update':{'sessionUpdate':'tool_call','toolCallId':'tool-1','title':'Create frame','status':'in_progress'}}})
  send({'jsonrpc':'2.0','id':99,'method':'session/request_permission','params':{'toolCall':{'title':'Create frame'},'options':[{'optionId':'allow','name':'Allow'}]}})
  prompt_id=msg['id']
 elif msg.get('id')==99:
  assert msg['result']['outcome']=={'outcome':'selected','optionId':'allow'}
  send({'jsonrpc':'2.0','method':'session/update','params':{'update':{'sessionUpdate':'tool_call_update','toolCallId':'tool-1','title':'Create frame','status':'completed'}}})
  send({'jsonrpc':'2.0','id':prompt_id,'result':{'stopReason':'end_turn'}})
"#).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&script, perms).unwrap();
        let agent =
            Agent::start_with_command(AgentKind::Codex, 42, 1, 7777, &folder.to_string_lossy(), "lizard-cli", Some(script.as_os_str())).unwrap();
        agent
            .send(AgentPrompt {
                text: "create a frame".into(),
                images: vec![lizard_ui_egui::agent_panel::AgentImage { mime: "image/png".into(), data: "aGVsbG8=".into() }],
                conversation: 1,
                resume: None,
                resumes: Vec::new(),
                model: None,
                agent_count: 1,
                parallel_mode: lizard_ui_egui::agent_panel::ParallelMode::Split,
                variant_count: 1,
                variant_mode: lizard_ui_egui::agent_panel::VariantMode::Style,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut got_text = false;
        let mut got_tool = false;
        let mut got_done = false;
        while Instant::now() < deadline && !got_done {
            for event in agent.poll() {
                match event {
                    AgentEvent::Text(text) => got_text |= text == "Working",
                    AgentEvent::Tool { status, .. } => got_tool |= status.as_deref() == Some("completed"),
                    AgentEvent::Permission { id, options, .. } => {
                        assert_eq!(options[0].0, "allow");
                        agent.permission(id, Some("allow".into()));
                    }
                    AgentEvent::Done(reason) => got_done = reason == "end_turn",
                    AgentEvent::Error(error) => panic!("{error}"),
                    AgentEvent::Ready
                    | AgentEvent::Session(_)
                    | AgentEvent::LoginUrl(_)
                    | AgentEvent::LoginComplete
                    | AgentEvent::LoginError(_)
                    | AgentEvent::Models(_)
                    | AgentEvent::Thinking(_)
                    | AgentEvent::Scoped { .. } => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got_text && got_tool && got_done);
        drop(agent);
        let resumed =
            Agent::start_with_command(AgentKind::Codex, 42, 1, 7777, &folder.to_string_lossy(), "lizard-cli", Some(script.as_os_str())).unwrap();
        resumed
            .send(AgentPrompt {
                text: "create a frame".into(),
                images: vec![lizard_ui_egui::agent_panel::AgentImage { mime: "image/png".into(), data: "aGVsbG8=".into() }],
                conversation: 1,
                resume: Some("session-1".into()),
                resumes: Vec::new(),
                model: None,
                agent_count: 1,
                parallel_mode: lizard_ui_egui::agent_panel::ParallelMode::Split,
                variant_count: 1,
                variant_mode: lizard_ui_egui::agent_panel::VariantMode::Style,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut got_resumed_reply = false;
        while Instant::now() < deadline && !got_resumed_reply {
            for event in resumed.poll() {
                if let AgentEvent::Permission { id, .. } = event {
                    resumed.permission(id, Some("allow".into()));
                } else if let AgentEvent::Done(reason) = event {
                    got_resumed_reply = reason == "end_turn";
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got_resumed_reply);
        let _ = std::fs::remove_dir_all(folder);
    }
}

fn update(message: &Value) -> Option<AgentEvent> {
    let value = &message["params"]["update"];
    match value["sessionUpdate"].as_str()? {
        "agent_message_chunk" => value["content"]["text"].as_str().map(|text| AgentEvent::Text(text.chars().take(16_384).collect())),
        "agent_thought_chunk" => value["content"]["text"].as_str().map(|text| AgentEvent::Thinking(text.chars().take(16_384).collect())),
        "tool_call" | "tool_call_update" => Some(AgentEvent::Tool {
            id: value["toolCallId"].as_str()?.to_string(),
            title: value.get("title").and_then(Value::as_str).map(str::to_owned),
            status: value.get("status").and_then(Value::as_str).map(str::to_owned),
            input: value.get("rawInput").map(|entry| (!entry.is_null()).then(|| entry.clone())),
            output: value.get("rawOutput").map(|entry| (!entry.is_null()).then(|| entry.clone())),
            content: value.get("content").map(|entry| entry.as_array().cloned().unwrap_or_default()),
        }),
        _ => None,
    }
}
