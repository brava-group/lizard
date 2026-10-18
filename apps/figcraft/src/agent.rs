//! A small ACP v1 client for the Agent sidebar. The agent runs off the UI thread and receives
//! FigCraft's existing MCP server, attached to this window's control channel.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use figcraft_ui_egui::agent_panel::{AgentEvent, AgentKind, AgentPrompt};
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
}

impl Agent {
    pub fn start(kind: AgentKind, document: u64, port: u16, cwd: &str, cli: &str) -> Result<Self, String> {
        Self::start_with_command(kind, document, port, cwd, cli, std::env::var_os("FIGCRAFT_AGENT_COMMAND").as_deref())
    }

    fn start_with_command(
        kind: AgentKind,
        document: u64,
        port: u16,
        cwd: &str,
        cli: &str,
        override_command: Option<&std::ffi::OsStr>,
    ) -> Result<Self, String> {
        let (input, requests) = mpsc::channel();
        let (output, events) = mpsc::channel();
        let (program, args): (&str, &[&str]) = match kind {
            AgentKind::Codex => ("codex-acp", &[]),
            AgentKind::Claude => ("claude-agent-acp", &[]),
            AgentKind::Gemini => ("gemini", &["--acp"]),
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
        std::thread::Builder::new().name("figcraft-agent-read".into()).spawn(move || read_messages(stdout, read_tx)).map_err(|e| e.to_string())?;
        let mcp = json!({"name":"figcraft", "command":cli, "args":["mcp", "--attach", port.to_string()], "env":[]});
        let cwd = cwd.to_string();
        std::thread::Builder::new()
            .name("figcraft-agent".into())
            .spawn(move || run(child, stdin, requests, output, cwd, mcp))
            .map_err(|e| e.to_string())?;
        Ok(Self { input, events, document, kind })
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
        json!({"protocolVersion":1, "clientCapabilities":{}, "clientInfo":{"name":"figcraft", "title":"FigCraft", "version":env!("CARGO_PKG_VERSION")}}),
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
                    if method == "session/update" {
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
                        let reply = json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Método não disponível no FigCraft"}});
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
                                send(&mut stdin, Some(2), "session/new", json!({"cwd":cwd, "mcpServers":[mcp]}))
                            }
                        }
                        Some(2) => {
                            if let Some(id) = message["result"]["sessionId"].as_str() {
                                session = Some(id.to_string());
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    #[test]
    fn streams_tools_and_permission_through_acp() {
        let folder = std::env::temp_dir().join(format!("figcraft-acp-test-{}", std::process::id()));
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
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'protocolVersion':1,'agentCapabilities':{'promptCapabilities':{'image':True}}}})
 elif method=='session/new':
  assert msg['params']['mcpServers'][0]['name']=='figcraft'
  assert msg['params']['mcpServers'][0]['args']==['mcp','--attach','7777']
  send({'jsonrpc':'2.0','id':msg['id'],'result':{'sessionId':'session-1'}})
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
            Agent::start_with_command(AgentKind::Codex, 42, 7777, &folder.to_string_lossy(), "figcraft-cli", Some(script.as_os_str())).unwrap();
        agent
            .send(AgentPrompt {
                text: "create a frame".into(),
                images: vec![figcraft_ui_egui::agent_panel::AgentImage { mime: "image/png".into(), data: "aGVsbG8=".into() }],
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
                    AgentEvent::Tool { status, .. } => got_tool |= status == "completed",
                    AgentEvent::Permission { id, options, .. } => {
                        assert_eq!(options[0].0, "allow");
                        agent.permission(id, Some("allow".into()));
                    }
                    AgentEvent::Done(reason) => got_done = reason == "end_turn",
                    AgentEvent::Error(error) => panic!("{error}"),
                    AgentEvent::Ready => {}
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got_text && got_tool && got_done);
        let _ = std::fs::remove_dir_all(folder);
    }
}

fn update(message: &Value) -> Option<AgentEvent> {
    let value = &message["params"]["update"];
    match value["sessionUpdate"].as_str()? {
        "agent_message_chunk" => value["content"]["text"].as_str().map(|text| AgentEvent::Text(text.chars().take(16_384).collect())),
        "tool_call" | "tool_call_update" => Some(AgentEvent::Tool {
            id: value["toolCallId"].as_str()?.to_string(),
            title: value["title"].as_str().unwrap_or("Ação do agente").to_string(),
            status: value["status"].as_str().unwrap_or("in_progress").to_string(),
        }),
        _ => None,
    }
}
