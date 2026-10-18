//! A one-shot JavaScript worker. It receives a bounded document snapshot and returns typed edits;
//! it has no host functions, document handle, filesystem or network API.

use std::cell::Cell;
use std::io::{Read, Write};
use std::rc::Rc;

use boa_engine::builtins::promise::{OperationType, Promise};
use boa_engine::context::HostHooks;
use boa_engine::module::IdleModuleLoader;
use boa_engine::object::JsObject;
use boa_engine::{Context, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::{self, Edit};

pub const WORKER_ARG: &str = "--figcraft-figma-plugin-worker";
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_OPERATIONS: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SelectedNode {
    pub id: String,
    pub name: String,
    pub opacity: f32,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    #[serde(rename = "type")]
    pub node_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub source: String,
    pub command: Option<String>,
    pub selection: Vec<SelectedNode>,
    #[serde(default)]
    pub extra: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetOpacity {
    pub id: String,
    pub opacity: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
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

/// The desktop and CLI executables enter here before initializing their normal application state.
pub fn run_stdio() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin().take((MAX_REQUEST_BYTES + 1) as u64).read_to_end(&mut input).map_err(|e| format!("cannot read plugin request: {e}"))?;
    let answer: Result<Response, String> = if input.len() > MAX_REQUEST_BYTES {
        Err("plugin request is too large".into())
    } else {
        serde_json::from_slice::<Request>(&input).map_err(|e| format!("invalid plugin request: {e}")).and_then(execute)
    };
    let output = serde_json::to_vec(&answer).map_err(|e| format!("cannot encode plugin result: {e}"))?;
    if output.len() > MAX_RESPONSE_BYTES {
        return Err("plugin response is too large".into());
    }
    std::io::stdout().write_all(&output).map_err(|e| format!("cannot write plugin result: {e}"))
}

/// Also used by focused semantic tests. Production calls this inside a child process.
pub fn execute(request: Request) -> Result<Response, String> {
    if request.source.len() > MAX_REQUEST_BYTES / 2 || request.selection.len() > 4096 {
        return Err("plugin source or selection is too large".into());
    }
    let selection = serde_json::to_value(&request.selection).map_err(|e| e.to_string())?;
    let command = serde_json::to_value(&request.command).map_err(|e| e.to_string())?;
    let hooks = Rc::new(RestrictedHooks::default());
    let mut context =
        Context::builder().host_hooks(hooks.clone()).module_loader(Rc::new(IdleModuleLoader)).build().map_err(|e| format!("plugin setup: {e}"))?;
    context.runtime_limits_mut().set_loop_iteration_limit(100_000);
    context.runtime_limits_mut().set_recursion_limit(128);
    context.runtime_limits_mut().set_stack_size_limit(1024);
    let setup = bridge::setup(&selection, &request.extra, &command, &Value::Null, &Value::Null, MAX_OPERATIONS, 0);
    context.eval(Source::from_bytes(&setup)).map_err(|e| format!("plugin setup: {e}"))?;
    context.eval(Source::from_bytes(&request.source)).map_err(|e| format!("plugin JavaScript: {e}"))?;
    context.run_jobs().map_err(|e| format!("plugin JavaScript task: {e}"))?;
    context.eval(Source::from_bytes("__figcraftRun()")).map_err(|e| format!("plugin event: {e}"))?;
    context.run_jobs().map_err(|e| format!("plugin JavaScript task: {e}"))?;
    bridge::run_timers(&mut context)?;
    if hooks.rejected_promise.get() {
        return Err("plugin Promise was rejected".into());
    }
    let value = context.eval(Source::from_bytes("__figcraftTake()")).map_err(|e| format!("plugin result: {e}"))?;
    let output = value.to_string(&mut context).map_err(|e| format!("plugin result: {e}"))?.to_std_string_escaped();
    if output.len() > MAX_RESPONSE_BYTES {
        return Err("plugin response is too large".into());
    }
    let response: Response = serde_json::from_str(&output).map_err(|e| format!("invalid plugin result: {e}"))?;
    if response.operations.len() + response.edits.len() > MAX_OPERATIONS || response.message.as_ref().is_some_and(|v| v.len() > 4096) {
        return Err("plugin operation or message limit exceeded".into());
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(source: &str) -> Request {
        Request {
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
        }
    }

    #[test]
    fn selected_node_reads_and_writes() {
        let result = execute(request("const n = figma.currentPage.selection[0]; if (n.name !== 'Card') throw Error('name'); n.opacity = 0.4; if (n.opacity !== 0.4) throw Error('opacity'); figma.closePlugin('Done')"))
            .expect("valid plugin");
        assert_eq!(result.operations[0].id, "42");
        assert_eq!(result.operations[0].opacity, 0.4);
        assert!(result.closed);
        assert_eq!(result.message.as_deref(), Some("Done"));
    }

    #[test]
    fn error_after_write_does_not_return_operations() {
        assert!(execute(request("figma.currentPage.selection[0].opacity = 0.4; throw Error('stop')")).is_err());
    }

    #[test]
    fn loop_and_invalid_opacity_are_bounded() {
        assert!(execute(request("while (true) {}")).is_err());
        assert!(execute(request("figma.currentPage.selection[0].opacity = Infinity")).is_err());
    }

    #[test]
    fn queued_promise_can_finish_before_close() {
        let result = execute(request("Promise.resolve().then(() => { figma.currentPage.selection[0].opacity = 0.7; figma.closePlugin(); })"))
            .expect("resolved promise");
        assert!(result.closed);
        assert_eq!(result.operations[0].opacity, 0.7);
    }

    #[test]
    fn rejected_promise_after_write_discards_all_edits() {
        assert!(
            execute(request(
                "Promise.resolve().then(() => { figma.currentPage.selection[0].opacity = 0.2; throw Error('failed'); }); figma.closePlugin()"
            ))
            .is_err()
        );
    }

    #[test]
    fn module_import_cannot_read_files() {
        assert!(execute(request("import('./secret.js'); figma.closePlugin()")).is_err());
    }

    #[test]
    fn creates_editable_nodes_and_selection() {
        let result = execute(request("const r = figma.createRectangle(); r.x = 42; r.fills = [{type:'SOLID',color:{r:1,g:0,b:0}}]; figma.currentPage.appendChild(r); figma.currentPage.selection = [r]; figma.viewport.scrollAndZoomIntoView([r]); figma.closePlugin()"))
            .expect("rectangle plugin");
        assert!(result.closed);
        assert_eq!(result.edits.len(), 6);
        assert!(matches!(&result.edits[0], Edit::CreateNode { node_type, .. } if node_type == "rectangle"));
        assert!(matches!(&result.edits[4], Edit::Select { ids } if ids.len() == 1));
    }

    #[test]
    fn text_review_event_returns_suggestions() {
        let mut req = request("figma.on('textreview', ({text}) => [{start: text.indexOf('figma'), end: 5, suggestions: ['Figma']}])");
        req.command = Some("textreview".into());
        req.extra = serde_json::json!({"text":"figma"});
        let result = execute(req).expect("text review callback");
        assert_eq!(result.event_result.unwrap_or(Value::Null)[0]["suggestions"][0], "Figma");
    }

    #[test]
    fn parameter_input_returns_suggestions_and_errors() {
        let mut req = request(
            "figma.parameters.on('input', ({query, result}) => { if (query === '0') result.setError('positive only'); else result.setSuggestions(['640','800']); })",
        );
        req.command = Some("absolute".into());
        req.extra = serde_json::json!({"parameterInput":{"key":"width","query":"0"}});
        let error = execute(req.clone()).expect("parameter input");
        assert_eq!(error.event_result.unwrap_or(Value::Null)["error"], "positive only");
        req.extra = serde_json::json!({"parameterInput":{"key":"width","query":""}});
        let choices = execute(req).expect("parameter suggestions");
        assert_eq!(choices.event_result.unwrap_or(Value::Null)["suggestions"], serde_json::json!(["640", "800"]));
    }
}
