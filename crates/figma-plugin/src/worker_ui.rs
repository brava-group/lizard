//! Persistent, bounded JavaScript worker for a local plugin with HTML UI.
//! Each UI message is one transaction: an exception discards that message's edits.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::rc::Rc;

use base64::Engine as _;
use boa_engine::builtins::promise::{OperationType, Promise, PromiseState};
use boa_engine::context::HostHooks;
use boa_engine::module::IdleModuleLoader;
use boa_engine::object::{JsObject, builtins::JsPromise};
use boa_engine::{Context, Source};
use boa_engine::{JsValue, NativeFunction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::Edit;
use crate::worker::{MAX_OPERATIONS, SelectedNode, SetOpacity};

pub const WORKER_UI_ARG: &str = "--figcraft-figma-plugin-ui-worker";
pub const MAX_UI_HTML_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_UI_SOURCE_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_UI_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_UI_RESPONSE_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_UI_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CALLBACKS: usize = 1024;

thread_local! {
    static ALLOWED_DOMAINS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn network_host(url: &str, allowed: &[String]) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or("plugin network requests require HTTPS")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() || host.contains(['@', ':', '%']) || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')) {
        return Err("plugin network URL has an invalid host".into());
    }
    if host == "localhost" || host.ends_with(".localhost") || host.parse::<std::net::IpAddr>().is_ok() {
        return Err("plugin network URL cannot target a local address".into());
    }
    let permitted = allowed.iter().any(|entry| {
        if entry == "*" {
            return true;
        }
        let Some(origin) = entry.strip_prefix("https://") else { return false };
        let listed = origin.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        if let Some(suffix) = listed.strip_prefix("*.") { host.ends_with(&format!(".{suffix}")) } else { host == listed }
    });
    if !permitted {
        return Err("plugin network URL is not declared in its manifest".into());
    }
    Ok(())
}

fn perform_fetch(url: &str, options: &Value) -> Result<Value, String> {
    ALLOWED_DOMAINS.with(|domains| network_host(url, &domains.borrow()))?;
    let method = options["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build();
    let agent: ureq::Agent = config.into();
    let headers = options["headers"].as_object();
    let body = options["body"].as_str().unwrap_or_default();
    let mut response = match method.as_str() {
        "GET" => {
            let mut request = agent.get(url);
            if let Some(headers) = headers {
                for (name, value) in headers {
                    if let Some(value) = value.as_str() {
                        request = request.header(name.as_str(), value);
                    }
                }
            }
            request.call().map_err(|e| e.to_string())?
        }
        "POST" => {
            if body.len() > 2 * 1024 * 1024 {
                return Err("plugin request body is too large".into());
            }
            let mut request = agent.post(url);
            if let Some(headers) = headers {
                for (name, value) in headers {
                    if let Some(value) = value.as_str() {
                        request = request.header(name.as_str(), value);
                    }
                }
            }
            request.send(body.as_bytes()).map_err(|e| e.to_string())?
        }
        _ => return Err("plugin network method is not supported".into()),
    };
    let status = response.status().as_u16();
    let mut response_headers = serde_json::Map::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            response_headers.insert(name.as_str().to_ascii_lowercase(), Value::String(value.to_owned()));
        }
    }
    let bytes = response.body_mut().with_config().limit(8 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(serde_json::json!({
        "status": status,
        "ok": (200..300).contains(&status),
        "headers": response_headers,
        "text": text,
        "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

fn fetch_native(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let url = args.first().cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    let options = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    let options: Value = serde_json::from_str(&options).unwrap_or(Value::Null);
    let result = match perform_fetch(&url, &options) {
        Ok(value) => value,
        Err(error) => serde_json::json!({"error":error}),
    };
    let encoded = serde_json::to_string(&result).unwrap_or_else(|_| "{\"error\":\"cannot encode network response\"}".into());
    Ok(JsValue::from(boa_engine::JsString::from(encoded)))
}

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
        #[serde(default)]
        allowed_domains: Vec<String>,
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
    pub warnings: Vec<String>,
    #[serde(default)]
    pub storage: BTreeMap<String, Option<Value>>,
    #[serde(default)]
    #[serde(rename = "eventResult")]
    pub event_result: Option<Value>,
}

#[derive(Default)]
struct RestrictedHooks {
    rejected_promise: RefCell<Option<String>>,
}

impl HostHooks for RestrictedHooks {
    fn promise_rejection_tracker(&self, promise: &JsObject<Promise>, operation: OperationType, context: &mut Context) {
        if operation == OperationType::Reject
            && let PromiseState::Rejected(reason) = JsPromise::from(promise.clone()).state()
        {
            let summary = reason.to_string(context).map_or_else(|_| "unknown reason".to_string(), |s| s.to_std_string_escaped());
            let stack = reason
                .as_object()
                .and_then(|object| object.get(boa_engine::js_string!("stack"), context).ok())
                .filter(|stack| !stack.is_undefined())
                .unwrap_or(reason)
                .to_string(context)
                .map_or_else(|_| "unknown reason".to_string(), |s| s.to_std_string_escaped());
            *self.rejected_promise.borrow_mut() = Some(format!("{summary}\n{stack}").chars().take(1024).collect());
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
        let UiRequest::Start { source, command, selection, extra, html, named_html, allowed_domains } = request else {
            return Err("the first UI worker request must start a plugin".into());
        };
        if source.len() > MAX_UI_SOURCE_BYTES
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
        ALLOWED_DOMAINS.with(|domains| *domains.borrow_mut() = allowed_domains);
        context
            .register_global_builtin_callable(boa_engine::js_string!("__figcraftFetch"), 2, NativeFunction::from_fn_ptr(fetch_native))
            .map_err(|e| format!("plugin network setup: {e}"))?;
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
            source.starts_with("const __html__ =") || source.starts_with("const __uiFiles__ ="),
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
        let value = self.context.eval(Source::from_bytes("__figcraftTake()")).map_err(|e| format!("plugin UI result: {e}"))?;
        let output = value.to_string(&mut self.context).map_err(|e| format!("plugin UI result: {e}"))?.to_std_string_escaped();
        if output.len() > MAX_UI_RESPONSE_BYTES {
            return Err("plugin UI response is too large".into());
        }
        let mut response: UiResponse = serde_json::from_str(&output).map_err(|e| format!("invalid plugin UI result: {e}"))?;
        if let Some(reason) = self.hooks.rejected_promise.borrow_mut().take() {
            if response.show.is_some() && response.edits.is_empty() && response.operations.is_empty() {
                response.warnings.push(format!("plugin startup Promise was rejected: {reason}"));
            } else {
                return Err(format!("plugin Promise was rejected: {reason}"));
            }
        }
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

    #[test]
    fn worker_network_grant_stays_on_declared_https_hosts() {
        let domains = vec!["https://*.icons8.com".to_string()];
        assert!(network_host("https://api.icons8.com/icons", &domains).is_ok());
        assert!(network_host("https://icons8.com/icons", &domains).is_err());
        assert!(network_host("http://api.icons8.com/icons", &domains).is_err());
        assert!(network_host("https://api.icons8.com.evil.test/icons", &domains).is_err());
        assert!(network_host("https://localhost/admin", &["*".to_string()]).is_err());
    }

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
                children: Vec::new(),
                characters: None,
                fills: Vec::new(),
            }],
            extra: Value::Null,
            html: Some("<button>Apply</button>".into()),
            named_html: BTreeMap::new(),
            allowed_domains: Vec::new(),
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
    fn published_bundle_can_declare_its_own_html_and_use_storage_and_messages() {
        let source = "const __html__ = '<p>Bundled UI</p>'; figma.clientStorage.setAsync('ready', true).then(() => figma.showUI(__html__)); figma.ui.on('message', message => { figma.ui.postMessage({ready: message.ready}); });";
        let (mut runtime, shown) = UiRuntime::start(start(source)).expect("published bundle");
        assert_eq!(shown.show.as_ref().map(|ui| ui.html.as_str()), Some("<p>Bundled UI</p>"));
        assert_eq!(shown.storage.get("ready"), Some(&Some(json!(true))));
        let reply = runtime.receive(&json!({"ready":true})).expect("UI message");
        assert_eq!(reply.messages, [json!({"ready":true})]);
    }

    #[test]
    fn client_storage_reads_previous_values_and_reports_updates() {
        let mut request = start(
            "figma.clientStorage.getAsync('token').then(value => figma.showUI(String(value))); figma.ui.onmessage = () => figma.clientStorage.setAsync('token', 'new');",
        );
        if let UiRequest::Start { extra, .. } = &mut request {
            *extra = json!({"clientStorage":{"token":"old"}});
        }
        let (mut runtime, shown) = UiRuntime::start(request).expect("UI start");
        assert_eq!(shown.show.map(|ui| ui.html).as_deref(), Some("old"));
        let reply = runtime.receive(&json!({})).expect("storage update");
        assert_eq!(reply.storage.get("token"), Some(&Some(json!("new"))));
    }

    #[test]
    fn selected_node_can_export_a_rest_snapshot_for_inspection_plugins() {
        let source = "figma.showUI(__html__); figma.ui.onmessage = () => figma.currentPage.selection[0].exportAsync({format:'JSON_REST_V1'}).then(value => figma.ui.postMessage(value));";
        let (mut runtime, _) = UiRuntime::start(start(source)).expect("UI start");
        let reply = runtime.receive(&json!({})).expect("selection export");
        assert_eq!(reply.messages[0]["document"]["name"], "Card");
        assert_eq!(reply.messages[0]["document"]["absoluteBoundingBox"]["width"], 100);
    }

    #[test]
    fn existing_image_bytes_are_available_to_the_plugin() {
        let source = "figma.showUI(__html__); figma.ui.onmessage = () => { figma.commitUndo(); return figma.getImageByHash('image-hash').getBytesAsync().then(bytes => figma.ui.postMessage(Array.from(bytes))); };";
        let mut request = start(source);
        if let UiRequest::Start { extra, .. } = &mut request {
            *extra = json!({"images":{"image-hash":{"data":"AQID","width":1,"height":1}}});
        }
        let (mut runtime, _) = UiRuntime::start(request).expect("UI start");
        let reply = runtime.receive(&json!({})).expect("image read");
        assert_eq!(reply.messages, [json!([1, 2, 3])]);
    }

    #[test]
    fn typed_image_bytes_survive_ui_message_roundtrip() {
        let source = "figma.showUI(__html__); figma.ui.onmessage = message => figma.ui.postMessage({first: message.bytes[0], bytes: new Uint8Array([1,2,3])});";
        let (mut runtime, _) = UiRuntime::start(start(source)).expect("UI start");
        let reply = runtime.receive(&json!({"bytes":{"__figcraftBytes":"BAUG"}})).expect("binary UI message");
        assert_eq!(reply.messages, [json!({"first":4,"bytes":{"__figcraftBytes":"AQID"}})]);
    }

    #[test]
    fn published_bundle_can_use_legacy_substr_during_initialization() {
        let (_, shown) = UiRuntime::start(start("const __html__ = '*Ready'.substr(1); figma.showUI(__html__);")).expect("legacy bundle");
        assert_eq!(shown.show.map(|ui| ui.html).as_deref(), Some("Ready"));
    }

    #[test]
    fn ui_resize_after_initial_show_keeps_the_open_window() {
        let (mut runtime, shown) =
            UiRuntime::start(start("figma.showUI(__html__); figma.ui.onmessage = () => { figma.ui.resize(480, 320); figma.ui.show(); };"))
                .expect("UI start");
        assert!(shown.show.is_some());
        let reply = runtime.receive(&json!({"type":"ready"})).expect("resize from UI");
        assert_eq!(reply.show.map(|ui| (ui.width, ui.height)), Some((480, 320)));
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
