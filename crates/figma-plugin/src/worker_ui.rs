//! Persistent, bounded JavaScript worker for a local plugin with HTML UI.
//! Each UI message is one transaction: an exception discards that message's edits.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::rc::Rc;

use boa_engine::builtins::promise::{OperationType, Promise};
use boa_engine::context::HostHooks;
use boa_engine::module::IdleModuleLoader;
use boa_engine::object::JsObject;
use boa_engine::{Context, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::Edit;
use crate::worker::{MAX_OPERATIONS, SelectedNode, SetOpacity};

pub const WORKER_UI_ARG: &str = "--figcraft-figma-plugin-ui-worker";
pub const MAX_UI_HTML_BYTES: usize = 512 * 1024;
pub const MAX_UI_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_UI_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_UI_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_CALLBACKS: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UiRequest {
    Start {
        source: String,
        command: Option<String>,
        selection: Vec<SelectedNode>,
        extra: Value,
        html: Option<String>,
        named_html: BTreeMap<String, String>,
    },
    Message {
        payload: Value,
    },
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiShow {
    pub html: String,
    pub width: u32,
    pub height: u32,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiResponse {
    pub show: Option<UiShow>,
    pub messages: Vec<Value>,
    pub operations: Vec<SetOpacity>,
    #[serde(default)]
    pub edits: Vec<Edit>,
    pub closed: bool,
    pub message: Option<String>,
    #[serde(default)]
    #[serde(rename = "eventResult")]
    pub event_result: Option<Value>,
}

#[derive(Default)]
struct RestrictedHooks {
    rejected_promise: Cell<bool>,
}

impl HostHooks for RestrictedHooks {
    fn promise_rejection_tracker(&self, _promise: &JsObject<Promise>, operation: OperationType, _context: &mut Context) {
        if operation == OperationType::Reject {
            self.rejected_promise.set(true);
        }
    }

    fn max_buffer_size(&self, _context: &mut Context) -> u64 {
        16 * 1024 * 1024 * 8
    }
}

/// One JS context stays alive across messages; it never owns a document or filesystem handle.
pub struct UiRuntime {
    context: Context,
    hooks: Rc<RestrictedHooks>,
    callbacks: usize,
    closed: bool,
}

impl UiRuntime {
    pub fn start(request: UiRequest) -> Result<(Self, UiResponse), String> {
        let UiRequest::Start { source, command, selection, extra, html, named_html } = request else {
            return Err("the first UI worker request must start a plugin".into());
        };
        if source.len() > crate::worker::MAX_REQUEST_BYTES / 2
            || selection.len() > 4096
            || html.as_ref().is_some_and(|v| v.len() > MAX_UI_HTML_BYTES)
            || named_html.len() > 32
            || named_html.values().any(|v| v.len() > MAX_UI_HTML_BYTES)
            || named_html.values().map(String::len).sum::<usize>() > MAX_UI_HTML_BYTES
        {
            return Err("plugin source, selection or HTML is too large".into());
        }
        let hooks = Rc::new(RestrictedHooks::default());
        let mut context = Context::builder()
            .host_hooks(hooks.clone())
            .module_loader(Rc::new(IdleModuleLoader))
            .build()
            .map_err(|e| format!("plugin setup: {e}"))?;
        context.runtime_limits_mut().set_loop_iteration_limit(100_000);
        context.runtime_limits_mut().set_recursion_limit(128);
        context.runtime_limits_mut().set_stack_size_limit(1024);
        let setup = crate::bridge::setup(
            &serde_json::to_value(&selection).map_err(|e| e.to_string())?,
            &extra,
            &serde_json::to_value(&command).map_err(|e| e.to_string())?,
            &serde_json::to_value(&html).map_err(|e| e.to_string())?,
            &serde_json::to_value(&named_html).map_err(|e| e.to_string())?,
            MAX_OPERATIONS,
            MAX_UI_HTML_BYTES,
        );
        context.eval(Source::from_bytes(&setup)).map_err(|e| format!("plugin setup: {e}"))?;
        context.eval(Source::from_bytes(&source)).map_err(|e| format!("plugin JavaScript: {e}"))?;
        context.run_jobs().map_err(|e| format!("plugin JavaScript task: {e}"))?;
        context.eval(Source::from_bytes("__figcraftRun()")).map_err(|e| format!("plugin event: {e}"))?;
        let mut runtime = Self { context, hooks, callbacks: 0, closed: false };
        let response = runtime.finish_callback()?;
        if response.show.is_none() && !response.closed {
            return Err("plugin did not call figma.showUI() or figma.closePlugin()".into());
        }
        Ok((runtime, response))
    }

    pub fn receive(&mut self, payload: &Value) -> Result<UiResponse, String> {
        if self.closed {
            return Err("plugin is closed".into());
        }
        if self.callbacks >= MAX_CALLBACKS {
            return Err("plugin callback limit exceeded".into());
        }
        let encoded = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        if encoded.len() > MAX_UI_MESSAGE_BYTES {
            return Err("plugin UI message is too large".into());
        }
        self.callbacks += 1;
        self.context.runtime_limits_mut().set_loop_iteration_limit(100_000);
        self.context.eval(Source::from_bytes(&format!("__figcraftReceive({encoded})"))).map_err(|e| format!("plugin UI callback: {e}"))?;
        self.finish_callback()
    }

    fn finish_callback(&mut self) -> Result<UiResponse, String> {
        self.context.run_jobs().map_err(|e| format!("plugin JavaScript task: {e}"))?;
        crate::bridge::run_timers(&mut self.context)?;
        if self.hooks.rejected_promise.get() {
            return Err("plugin Promise was rejected".into());
        }
        let value = self.context.eval(Source::from_bytes("__figcraftTake()")).map_err(|e| format!("plugin UI result: {e}"))?;
        let output = value.to_string(&mut self.context).map_err(|e| format!("plugin UI result: {e}"))?.to_std_string_escaped();
        if output.len() > MAX_UI_RESPONSE_BYTES {
            return Err("plugin UI response is too large".into());
        }
        let response: UiResponse = serde_json::from_str(&output).map_err(|e| format!("invalid plugin UI result: {e}"))?;
        if response.operations.len() + response.edits.len() > MAX_OPERATIONS
            || response.messages.len() > 128
            || response.messages.iter().any(|v| serde_json::to_vec(v).is_ok_and(|bytes| bytes.len() > MAX_UI_MESSAGE_BYTES))
            || response.show.as_ref().is_some_and(|v| v.html.len() > MAX_UI_HTML_BYTES)
        {
            return Err("plugin UI output limit exceeded".into());
        }
        self.closed = response.closed;
        Ok(response)
    }
}

fn read_line_capped(reader: &mut impl Read) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte).map_err(|e| format!("cannot read plugin UI request: {e}"))? {
            0 if line.is_empty() => return Ok(None),
            0 => return Err("incomplete plugin UI request".into()),
            _ if byte[0] == b'\n' => return Ok(Some(line)),
            _ if line.len() < MAX_UI_REQUEST_BYTES => line.push(byte[0]),
            _ => return Err("plugin UI request is too large".into()),
        }
    }
}

fn write_result(writer: &mut impl Write, result: &Result<UiResponse, String>) -> Result<(), String> {
    let bytes = serde_json::to_vec(result).map_err(|e| format!("cannot encode plugin UI result: {e}"))?;
    if bytes.len() > MAX_UI_RESPONSE_BYTES {
        return Err("plugin UI response is too large".into());
    }
    writer
        .write_all(&bytes)
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(|e| format!("cannot write plugin UI result: {e}"))
}

pub fn run_stdio() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let first = read_line_capped(&mut reader)?.ok_or("missing plugin UI start request")?;
    let request: UiRequest = serde_json::from_slice(&first).map_err(|e| format!("invalid plugin UI request: {e}"))?;
    let (mut runtime, response) = match UiRuntime::start(request) {
        Ok(value) => value,
        Err(error) => {
            write_result(&mut writer, &Err(error))?;
            return Ok(());
        }
    };
    let mut closed = response.closed;
    write_result(&mut writer, &Ok(response))?;
    while !closed {
        let Some(line) = read_line_capped(&mut reader)? else { break };
        let request: UiRequest = serde_json::from_slice(&line).map_err(|e| format!("invalid plugin UI request: {e}"))?;
        match request {
            UiRequest::Message { payload } => {
                let answer = runtime.receive(&payload);
                closed = answer.as_ref().map_or(true, |v| v.closed);
                write_result(&mut writer, &answer)?;
            }
            UiRequest::Stop => break,
            UiRequest::Start { .. } => {
                write_result(&mut writer, &Err("plugin UI session is already running".into()))?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn start(source: &str) -> UiRequest {
        UiRequest::Start {
            source: source.into(),
            command: None,
            selection: vec![SelectedNode {
                id: "42".into(),
                name: "Card".into(),
                opacity: 1.0,
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
                node_type: "RECTANGLE".into(),
            }],
            extra: Value::Null,
            html: Some("<button>Apply</button>".into()),
            named_html: BTreeMap::new(),
        }
    }

    #[test]
    fn html_message_and_close_keep_one_context() {
        let (mut runtime, shown) = UiRuntime::start(start(
            "figma.showUI(__html__, { width: 360, height: 240 }); figma.ui.onmessage = msg => { if (msg.kind === 'apply') { figma.currentPage.selection[0].opacity = msg.opacity; figma.ui.postMessage({ ok: true }); } else figma.closePlugin('Done'); }",
        ))
        .unwrap();
        assert_eq!(shown.show.unwrap().html, "<button>Apply</button>");
        let reply = runtime.receive(&json!({"kind":"apply","opacity":0.35})).unwrap();
        assert_eq!(reply.operations[0].opacity, 0.35);
        assert_eq!(reply.messages, [json!({"ok":true})]);
        assert!(!reply.closed);
        let done = runtime.receive(&json!({"kind":"close"})).unwrap();
        assert!(done.closed);
        assert_eq!(done.message.as_deref(), Some("Done"));
        assert!(runtime.receive(&json!(null)).is_err());
    }

    #[test]
    fn failed_callback_does_not_publish_edits() {
        let (mut runtime, _) = UiRuntime::start(start(
            "figma.showUI(__html__); figma.ui.onmessage = () => { figma.currentPage.selection[0].opacity = 0.2; throw Error('stop'); }",
        ))
        .unwrap();
        assert!(runtime.receive(&json!({})).is_err());
    }

    #[test]
    fn ui_scripts_are_bounded() {
        assert!(UiRuntime::start(start("figma.showUI(__html__); while(true) {}")).is_err());
        let (mut runtime, _) = UiRuntime::start(start("figma.showUI(__html__); figma.ui.onmessage = () => { while(true) {} };")).unwrap();
        assert!(runtime.receive(&json!({})).is_err());
    }

    #[test]
    fn ui_message_can_create_a_frame_and_reply_after_timer() {
        let (mut runtime, shown) = UiRuntime::start(start("figma.showUI(__html__); figma.ui.onmessage = values => { const frame = figma.createFrame(); frame.resizeWithoutConstraints(800,600); const rect = figma.createRectangle(); frame.appendChild(rect); rect.fills = [{type:'SOLID',color:{r:1,g:0,b:0}}]; setTimeout(() => figma.ui.postMessage({count:values.length}), 10); figma.closePlugin(); }"))
            .expect("UI plugin");
        assert!(shown.show.is_some());
        let started = std::time::Instant::now();
        let reply = runtime.receive(&json!([1, 2, 3])).expect("UI callback");
        assert!(started.elapsed() >= std::time::Duration::from_millis(8));
        assert_eq!(reply.messages, [json!({"count":3})]);
        assert!(reply.closed);
        assert_eq!(reply.edits.len(), 5);
    }
}
