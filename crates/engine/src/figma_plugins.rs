//! Local Figma JavaScript plugin diagnosis and a small, bounded execution slice. This is separate
//! from `plugin.import`, which only accepts FigCraft's metered WebAssembly plugins.

#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Child, ChildStdin, Command, Stdio};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use figcraft_color::PaintKind;
#[cfg(not(target_arch = "wasm32"))]
use figcraft_doc::{NodeId, NodeKind, StyleKind};
#[cfg(not(target_arch = "wasm32"))]
use figcraft_figma_plugin::UiFiles;
#[cfg(not(target_arch = "wasm32"))]
use figcraft_figma_plugin::bridge::Edit;
#[cfg(not(target_arch = "wasm32"))]
use figcraft_figma_plugin::worker::{self, Request, Response, SelectedNode};
#[cfg(not(target_arch = "wasm32"))]
use figcraft_figma_plugin::worker_ui::{self, UiRequest, UiResponse};
use figcraft_figma_plugin::{MANIFEST_FILE, MAX_FILE_BYTES, MAX_MANIFEST_BYTES, MAX_PACKAGE_BYTES, ParsedManifest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cmd::CommandSpec;
use crate::params::string;
use crate::{Result, Session, bad, file_access, spec};

const COMMAND: &str = "plugin.figma.inspect";

#[cfg(not(target_arch = "wasm32"))]
fn snapshot(s: &Session, params: &Value) -> Result<(Vec<SelectedNode>, Value)> {
    let doc = s.doc()?;
    let selected = s
        .selection()
        .into_iter()
        .filter_map(|id| {
            let node = doc.node(id)?;
            let at = node.transform.translation();
            let node_type = match &node.kind {
                NodeKind::Text(_) => "TEXT",
                NodeKind::Frame => "FRAME",
                _ => "RECTANGLE",
            };
            Some(SelectedNode {
                id: id.to_string(),
                name: node.name.clone(),
                opacity: node.opacity,
                x: at.x,
                y: at.y,
                width: node.width,
                height: node.height,
                node_type: node_type.into(),
            })
        })
        .collect();
    let styles: Vec<Value> = doc
        .styles
        .iter()
        .filter_map(|style| {
            let StyleKind::Paint { paints } = &style.kind else { return None };
            let paints: Vec<Value> = paints
                .iter()
                .filter_map(|paint| {
                    let PaintKind::Solid { color } = &paint.kind else { return None };
                    Some(json!({"type":"SOLID", "visible":paint.visible, "blendMode":"NORMAL",
                "color":{"r":color.r,"g":color.g,"b":color.b}, "opacity":paint.opacity * color.a}))
                })
                .collect();
            Some(json!({"id":style.id,"name":style.name,"paints":paints}))
        })
        .collect();
    let view = &s.ds()?.view;
    let text = params.get("text").cloned().or_else(|| {
        s.selection()
            .first()
            .and_then(|id| doc.node(*id))
            .and_then(|node| if let NodeKind::Text(text) = &node.kind { Some(json!(text.characters)) } else { None })
    });
    let extra = json!({"center":{"x":view.cx,"y":view.cy},"styles":styles,
        "parameters":params.get("parameters"), "parameterInput":params.get("parameterInput"), "text":text});
    Ok((selected, extra))
}

#[cfg(not(target_arch = "wasm32"))]
fn paint_value(value: &Value) -> Result<Value> {
    let items = value.as_array().ok_or_else(|| bad("plugin.figma.run", "paint list must be an array"))?;
    if items.len() > 128 {
        return Err(bad("plugin.figma.run", "paint list is too large"));
    }
    let mut out = Vec::with_capacity(items.len());
    for paint in items {
        if paint["type"] != "SOLID" {
            return Err(bad("plugin.figma.run", "only solid Figma paints are supported"));
        }
        let color = paint.get("color").ok_or_else(|| bad("plugin.figma.run", "solid paint has no color"))?;
        out.push(json!({"type":"solid", "color":color,
            "opacity":paint.get("opacity").and_then(Value::as_f64).unwrap_or(1.0),
            "visible":paint.get("visible").and_then(Value::as_bool).unwrap_or(true)}));
    }
    Ok(Value::Array(out))
}

#[cfg(not(target_arch = "wasm32"))]
fn node_prop(key: &str, value: &Value) -> Result<Value> {
    match key {
        "fills" | "strokes" => paint_value(value),
        "strokeAlign" | "textAlignHorizontal" | "textAlignVertical" => Ok(json!(value.as_str().unwrap_or_default().to_ascii_lowercase())),
        "constraints" => {
            let axis = |v: &Value, horizontal: bool| -> Value {
                let word = v.as_str().unwrap_or_default();
                json!(match (horizontal, word) {
                    (true, "MIN") => "left",
                    (true, "MAX") => "right",
                    (false, "MIN") => "top",
                    (false, "MAX") => "bottom",
                    (true, "STRETCH") => "leftRight",
                    (false, "STRETCH") => "topBottom",
                    (_, "CENTER") => "center",
                    (_, "SCALE") => "scale",
                    _ => word,
                })
            };
            Ok(json!({"horizontal":axis(&value["horizontal"], true),
                "vertical":axis(&value["vertical"], false)}))
        }
        _ => Ok(value.clone()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_edits(s: &mut Session, edits: &[Edit], allowed: &std::collections::BTreeMap<String, NodeId>, command: &str) -> Result<Vec<String>> {
    if edits.len() > worker::MAX_OPERATIONS {
        return Err(bad(command, "plugin operation limit exceeded"));
    }
    let mut nodes = allowed.clone();
    let mut collections = std::collections::BTreeMap::<String, String>::new();
    let mut variables = std::collections::BTreeMap::<String, String>::new();
    let mut modes = std::collections::BTreeMap::<String, String>::new();
    let mut notifications = Vec::new();
    let get_node = |nodes: &std::collections::BTreeMap<String, NodeId>, id: &str| {
        nodes.get(id).copied().ok_or_else(|| bad(command, format!("plugin node `{id}` is unavailable")))
    };
    for edit in edits {
        match edit {
            Edit::CreateNode { id, node_type } => {
                if !id.starts_with("plugin:") || nodes.contains_key(id) {
                    return Err(bad(command, "duplicate or invalid plugin node ID"));
                }
                if !matches!(node_type.as_str(), "rectangle" | "frame" | "text") {
                    return Err(bad(command, "unsupported plugin node type"));
                }
                let created = s.execute("node.create", &json!({"type":node_type,"select":false}))?;
                let nid = created["id"].as_u64().map(NodeId).ok_or_else(|| bad(command, "node creation returned no ID"))?;
                nodes.insert(id.clone(), nid);
            }
            Edit::SetNode { id, key, value } => {
                let nid = get_node(&nodes, id)?;
                let allowed_key = matches!(
                    key.as_str(),
                    "name"
                        | "opacity"
                        | "x"
                        | "y"
                        | "width"
                        | "height"
                        | "fills"
                        | "strokes"
                        | "strokeWeight"
                        | "strokeAlign"
                        | "constraints"
                        | "characters"
                        | "fontSize"
                        | "textAlignHorizontal"
                        | "textAlignVertical"
                );
                if !allowed_key {
                    return Err(bad(command, format!("unsupported plugin node property `{key}`")));
                }
                let value = node_prop(key, value)?;
                s.execute("node.set", &json!({"id":nid,"props":{key:value}}))?;
            }
            Edit::AppendChild { parent, id, x, y } => {
                let child = get_node(&nodes, id)?;
                let pid = if parent == "page" { s.page()? } else { get_node(&nodes, parent)? };
                if !x.is_finite() || !y.is_finite() {
                    return Err(bad(command, "invalid child position"));
                }
                s.execute("node.reparent", &json!({"ids":[child],"parent":pid}))?;
                s.execute("node.set", &json!({"id":child,"props":{"x":x,"y":y}}))?;
            }
            Edit::Resize { id, width, height } => {
                let nid = get_node(&nodes, id)?;
                if !width.is_finite() || !height.is_finite() || *width <= 0.0 || *height <= 0.0 {
                    return Err(bad(command, "invalid plugin resize"));
                }
                s.execute("transform.resize", &json!({"ids":[nid],"width":width,"height":height}))?;
            }
            Edit::Rescale { id, factor } => {
                let nid = get_node(&nodes, id)?;
                if !factor.is_finite() || *factor <= 0.0 || *factor > 1000.0 {
                    return Err(bad(command, "invalid plugin scale"));
                }
                s.execute("transform.scale", &json!({"ids":[nid],"factor":factor}))?;
            }
            Edit::Select { ids } | Edit::Zoom { ids } => {
                let ids: Vec<NodeId> = ids.iter().map(|id| get_node(&nodes, id)).collect::<Result<_>>()?;
                s.execute("select.set", &json!({"ids":ids}))?;
                if matches!(edit, Edit::Zoom { .. }) {
                    s.execute("view.zoomSelection", &json!({}))?;
                }
            }
            Edit::CreateCollection { id, name } => {
                if !id.starts_with("plugin:") || collections.contains_key(id) {
                    return Err(bad(command, "invalid collection ID"));
                }
                let result = s.execute("variable.createCollection", &json!({"name":name}))?;
                let cid = result["id"].as_str().ok_or_else(|| bad(command, "collection creation returned no ID"))?.to_string();
                let mid = s
                    .doc()?
                    .variables
                    .collection(&cid)
                    .and_then(|c| c.default_mode())
                    .cloned()
                    .ok_or_else(|| bad(command, "collection has no mode"))?;
                modes.insert(format!("{id}/mode1"), mid);
                collections.insert(id.clone(), cid);
            }
            Edit::RenameMode { collection, mode, name } => {
                let cid = collections.get(collection).ok_or_else(|| bad(command, "unknown plugin collection"))?;
                let mid = modes.get(mode).ok_or_else(|| bad(command, "unknown plugin mode"))?;
                s.execute("variable.renameMode", &json!({"collection":cid,"mode":mid,"name":name}))?;
            }
            Edit::CreateVariable { id, collection, name, kind } => {
                let cid = collections.get(collection).ok_or_else(|| bad(command, "unknown plugin collection"))?;
                let kind = kind.to_ascii_lowercase();
                let result = s.execute("variable.create", &json!({"collection":cid,"name":name,"type":kind}))?;
                let vid = result["id"].as_str().ok_or_else(|| bad(command, "variable creation returned no ID"))?;
                variables.insert(id.clone(), vid.to_string());
            }
            Edit::SetVariable { id, mode, value } => {
                let vid = variables.get(id).ok_or_else(|| bad(command, "unknown plugin variable"))?;
                let mid = modes.get(mode).ok_or_else(|| bad(command, "unknown plugin mode"))?;
                let value = if value["type"] == "VARIABLE_ALIAS" {
                    let alias = value["id"].as_str().and_then(|id| variables.get(id)).ok_or_else(|| bad(command, "unknown plugin variable alias"))?;
                    json!({"alias":alias})
                } else {
                    value.clone()
                };
                s.execute("variable.set", &json!({"variable":vid,"mode":mid,"value":value}))?;
            }
            Edit::Notify { message } => notifications.push(message.clone()),
        }
    }
    Ok(notifications)
}

/// A development source recorded in preferences. Import and status never run its JavaScript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalPlugin {
    pub id: String,
    pub name: String,
    pub path: String,
    pub digest: String,
}

fn inspect(_s: &mut Session, params: &Value) -> Result<Value> {
    let source = string(params, "path").ok_or_else(|| bad(COMMAND, "`path` required: a local manifest.json or its folder"))?;
    let path = PathBuf::from(crate::library::absolute_path(source.trim()));
    let manifest_path = if file_access::is_dir(&path) { path.join(MANIFEST_FILE) } else { path };
    if manifest_path.file_name().is_none_or(|name| name != MANIFEST_FILE) {
        return Err(bad(COMMAND, "choose a manifest.json or its folder"));
    }
    reject_link(&manifest_path)?;
    let root = manifest_path.parent().ok_or_else(|| bad(COMMAND, "manifest has no parent folder"))?;
    reject_link(root)?;
    let manifest_bytes = file_access::read_capped(&manifest_path, MAX_MANIFEST_BYTES)?;
    let parsed = ParsedManifest::parse(&manifest_bytes).map_err(|e| bad(COMMAND, e))?;
    let mut hasher = Sha256::new();
    hasher.update(&manifest_bytes);
    let mut total = manifest_bytes.len() as u64;
    let mut files = Vec::with_capacity(parsed.files.len());
    for relative in &parsed.files {
        let file = checked_file(root, relative)?;
        let bytes = file_access::read_capped(&file, MAX_FILE_BYTES)?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_PACKAGE_BYTES {
            return Err(bad(COMMAND, "referenced files exceed 32 MB"));
        }
        hasher.update(relative.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
        files.push(json!({"path": relative, "bytes": bytes.len()}));
    }
    let digest = format!("{:x}", hasher.finalize());
    let mut diagnostics = parsed.diagnostics;
    diagnostics
        .push("Local execution supports a bounded subset of Figma nodes, styles, variables and UI messages. Inspect never runs JavaScript".into());
    let executable = cfg!(not(target_arch = "wasm32"))
        && parsed.manifest.editor_type.iter().any(|v| v == "figma")
        && parsed.manifest.document_access.as_deref() == Some("dynamic-page")
        && parsed.manifest.api.split('.').next() == Some("1");
    Ok(json!({
        "status": "diagnosticado",
        "executable": executable,
        "name": parsed.manifest.name,
        "id": parsed.manifest.id,
        "api": parsed.manifest.api,
        "editorType": parsed.manifest.editor_type,
        "documentAccess": parsed.manifest.document_access,
        "networkAccess": parsed.manifest.network_access,
        "commands": parsed.commands,
        "capabilities": parsed.manifest.capabilities,
        "hasUi": parsed.manifest.ui.is_some(),
        "files": files,
        "digest": digest,
        "source": manifest_path,
        "diagnostics": diagnostics,
    }))
}

fn import(s: &mut Session, params: &Value) -> Result<Value> {
    let command = "plugin.figma.import";
    if string(params, "mode").as_deref() != Some("development") {
        return Err(bad(command, "only `mode: development` is available; installed snapshots are not implemented"));
    }
    let report = inspect(s, params)?;
    let entry = LocalPlugin {
        id: report["id"].as_str().unwrap_or_default().to_string(),
        name: report["name"].as_str().unwrap_or_default().to_string(),
        path: report["source"].as_str().unwrap_or_default().to_string(),
        digest: report["digest"].as_str().unwrap_or_default().to_string(),
    };
    let replaced = match s.prefs.figma_plugins.iter_mut().find(|old| old.id == entry.id) {
        Some(old) => {
            if *old != entry {
                *old = entry;
                s.plugins.unsaved = true;
            }
            true
        }
        None => {
            s.prefs.figma_plugins.push(entry);
            s.plugins.unsaved = true;
            false
        }
    };
    Ok(json!({"registered": true, "mode": "development", "replaced": replaced, "report": report}))
}

fn status(s: &mut Session, params: &Value) -> Result<Value> {
    let id = string(params, "id").ok_or_else(|| bad("plugin.figma.status", "`id` required"))?;
    let entry = s.prefs.figma_plugins.iter().find(|p| p.id == id).ok_or_else(|| bad("plugin.figma.status", "no such local Figma plugin"))?;
    let path = entry.path.clone();
    let installed_digest = entry.digest.clone();
    let mut report = inspect(s, &json!({"path": path}))?;
    report["changedSinceImport"] = json!(report["digest"].as_str() != Some(installed_digest.as_str()));
    report["mode"] = json!("development");
    Ok(report)
}

fn remove(s: &mut Session, params: &Value) -> Result<Value> {
    let id = string(params, "id").ok_or_else(|| bad("plugin.figma.remove", "`id` required"))?;
    #[cfg(not(target_arch = "wasm32"))]
    if s.figma_ui.as_ref().is_some_and(|open| open.id == id) {
        s.figma_ui = None;
    }
    let before = s.prefs.figma_plugins.len();
    s.prefs.figma_plugins.retain(|p| p.id != id);
    let removed = before != s.prefs.figma_plugins.len();
    s.plugins.unsaved |= removed;
    Ok(json!({"removed": removed, "id": id}))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct UiProcess {
    pub id: String,
    doc_uid: u64,
    allowed: std::collections::BTreeMap<String, figcraft_doc::NodeId>,
    allowed_domains: Vec<String>,
    child: Child,
    stdin: ChildStdin,
    replies: std::sync::mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for UiProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl UiProcess {
    fn spawn(
        id: String,
        doc_uid: u64,
        allowed: std::collections::BTreeMap<String, figcraft_doc::NodeId>,
        allowed_domains: Vec<String>,
    ) -> std::result::Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| format!("cannot locate plugin UI worker: {e}"))?;
        let mut child = Command::new(executable)
            .arg(worker_ui::WORKER_UI_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start plugin UI worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("plugin UI worker has no input")?;
        let stdout = child.stdout.take().ok_or("plugin UI worker has no output")?;
        let (sender, replies) = std::sync::mpsc::sync_channel(2);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result = (&mut reader)
                    .take((worker_ui::MAX_UI_RESPONSE_BYTES + 2) as u64)
                    .read_until(b'\n', &mut line)
                    .map_err(|e| format!("cannot read plugin UI reply: {e}"))
                    .and_then(|count| {
                        if count == 0 {
                            Err("plugin UI worker exited".into())
                        } else if line.len() > worker_ui::MAX_UI_RESPONSE_BYTES + 1 {
                            Err("plugin UI reply is too large".into())
                        } else {
                            Ok(line)
                        }
                    });
                let failed = result.is_err();
                if sender.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self { id, doc_uid, allowed, allowed_domains, child, stdin, replies })
    }

    fn exchange(&mut self, request: &UiRequest) -> std::result::Result<UiResponse, String> {
        let mut bytes = serde_json::to_vec(request).map_err(|e| format!("cannot encode plugin UI request: {e}"))?;
        if bytes.len() > worker_ui::MAX_UI_REQUEST_BYTES {
            return Err("plugin UI request is too large".into());
        }
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).and_then(|_| self.stdin.flush()).map_err(|e| format!("cannot send plugin UI request: {e}"))?;
        let line =
            self.replies.recv_timeout(Duration::from_secs(2)).map_err(|_| "plugin UI callback exceeded the 2-second time limit".to_string())??;
        serde_json::from_slice::<std::result::Result<UiResponse, String>>(&line).map_err(|e| format!("invalid plugin UI worker reply: {e}"))?
    }
}

#[cfg(target_arch = "wasm32")]
fn ui_start(_s: &mut Session, _params: &Value) -> Result<Value> {
    Err(crate::EngineError::Disabled("plugin.figma.ui.start".into(), "local Figma JavaScript UI workers are not available in the browser yet".into()))
}

#[cfg(target_arch = "wasm32")]
fn ui_message(_s: &mut Session, _params: &Value) -> Result<Value> {
    Err(crate::EngineError::Disabled(
        "plugin.figma.ui.message".into(),
        "local Figma JavaScript UI workers are not available in the browser yet".into(),
    ))
}

#[cfg(target_arch = "wasm32")]
fn ui_close(_s: &mut Session, _params: &Value) -> Result<Value> {
    Err(crate::EngineError::Disabled("plugin.figma.ui.close".into(), "local Figma JavaScript UI workers are not available in the browser yet".into()))
}

#[cfg(not(target_arch = "wasm32"))]
fn ui_start(s: &mut Session, params: &Value) -> Result<Value> {
    let command_id = "plugin.figma.ui.start";
    let id = string(params, "id").ok_or_else(|| bad(command_id, "`id` required"))?;
    let entry = s.prefs.figma_plugins.iter().find(|p| p.id == id).ok_or_else(|| bad(command_id, "no such local Figma plugin"))?.clone();
    let report = inspect(s, &json!({"path": entry.path}))?;
    if report["digest"].as_str() != Some(entry.digest.as_str()) {
        return Err(bad(command_id, "plugin files changed since import; inspect and import again"));
    }
    let manifest_path = PathBuf::from(&entry.path);
    let manifest_bytes = file_access::read_capped(&manifest_path, MAX_MANIFEST_BYTES)?;
    let parsed = ParsedManifest::parse(&manifest_bytes).map_err(|e| bad(command_id, e))?;
    if parsed.manifest.ui.is_none()
        || !parsed.manifest.editor_type.iter().any(|v| v == "figma")
        || parsed.manifest.document_access.as_deref() != Some("dynamic-page")
        || parsed.manifest.api.split('.').next() != Some("1")
    {
        return Err(bad(command_id, "this UI preview requires a Figma Design plugin with dynamic-page access and API 1"));
    }
    let menu_command = string(params, "command");
    if let Some(command) = menu_command.as_deref()
        && !parsed.commands.iter().any(|item| item.command == command)
    {
        return Err(bad(command_id, "unknown plugin menu command"));
    }
    let root = manifest_path.parent().ok_or_else(|| bad(command_id, "manifest has no parent folder"))?;
    let main_path = checked_file(root, &parsed.manifest.main)?;
    let source = String::from_utf8(file_access::read_capped(&main_path, (worker::MAX_REQUEST_BYTES / 2) as u64)?)
        .map_err(|_| bad(command_id, "main JavaScript must be UTF-8"))?;
    let mut html = None;
    let mut named_html = std::collections::BTreeMap::new();
    match parsed.manifest.ui.as_ref() {
        Some(UiFiles::One(relative)) => {
            html = Some(
                String::from_utf8(file_access::read_capped(&checked_file(root, relative)?, worker_ui::MAX_UI_HTML_BYTES as u64)?)
                    .map_err(|_| bad(command_id, "plugin HTML must be UTF-8"))?,
            );
        }
        Some(UiFiles::Named(named)) => {
            for (key, relative) in named {
                let content = String::from_utf8(file_access::read_capped(&checked_file(root, relative)?, worker_ui::MAX_UI_HTML_BYTES as u64)?)
                    .map_err(|_| bad(command_id, "plugin HTML must be UTF-8"))?;
                named_html.insert(key.clone(), content);
            }
        }
        None => return Err(bad(command_id, "plugin has no HTML UI")),
    }
    if inspect(s, &json!({"path": entry.path}))?["digest"].as_str() != Some(entry.digest.as_str()) {
        return Err(bad(command_id, "plugin files changed while preparing execution"));
    }
    let doc_uid = s.ds()?.uid;
    let (selected, extra) = snapshot(s, params)?;
    let allowed = selected.iter().filter_map(|node| node.id.parse::<figcraft_doc::NodeId>().ok().map(|id| (node.id.clone(), id))).collect();
    let request = UiRequest::Start { source, command: menu_command, selection: selected, extra, html, named_html };
    s.figma_ui = None;
    let allowed_domains = parsed.manifest.network_access.as_ref().map(|access| access.allowed_domains.clone()).unwrap_or_default();
    let mut process = UiProcess::spawn(id.clone(), doc_uid, allowed, allowed_domains).map_err(|e| bad(command_id, e))?;
    let reply = process.exchange(&request).map_err(|e| bad(command_id, e))?;
    let result = apply_ui_response(s, &process, reply, command_id)?;
    if result["closed"] != true {
        s.figma_ui = Some(process);
    }
    Ok(result)
}

#[cfg(not(target_arch = "wasm32"))]
fn ui_message(s: &mut Session, params: &Value) -> Result<Value> {
    let command_id = "plugin.figma.ui.message";
    let id = string(params, "id").ok_or_else(|| bad(command_id, "`id` required"))?;
    let payload = params.get("payload").ok_or_else(|| bad(command_id, "`payload` required"))?;
    let mut process = s.figma_ui.take().ok_or_else(|| bad(command_id, "no plugin UI is open"))?;
    if process.id != id {
        s.figma_ui = Some(process);
        return Err(bad(command_id, "another plugin UI is open"));
    }
    if s.ds()?.uid != process.doc_uid {
        return Err(bad(command_id, "the plugin's document is no longer active"));
    }
    let reply = process.exchange(&UiRequest::Message { payload: payload.clone() }).map_err(|e| bad(command_id, e))?;
    let result = apply_ui_response(s, &process, reply, command_id)?;
    if result["closed"] != true {
        s.figma_ui = Some(process);
    }
    Ok(result)
}

#[cfg(not(target_arch = "wasm32"))]
fn ui_close(s: &mut Session, _params: &Value) -> Result<Value> {
    let closed = s.figma_ui.take().is_some();
    Ok(json!({"closed": closed}))
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_ui_response(s: &mut Session, process: &UiProcess, reply: UiResponse, command_id: &str) -> Result<Value> {
    let mut result = apply_ui_operations(s, &process.id, &process.allowed, reply, command_id)?;
    if result["show"].is_object() {
        result["show"]["allowedDomains"] = json!(process.allowed_domains);
    }
    Ok(result)
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_ui_operations(
    s: &mut Session,
    id: &str,
    allowed: &std::collections::BTreeMap<String, figcraft_doc::NodeId>,
    reply: UiResponse,
    command_id: &str,
) -> Result<Value> {
    if reply.operations.len() > worker::MAX_OPERATIONS {
        return Err(bad(command_id, "plugin operation limit exceeded"));
    }
    for op in &reply.operations {
        if !op.opacity.is_finite() || !(0.0..=1.0).contains(&op.opacity) || !allowed.contains_key(&op.id) {
            return Err(bad(command_id, "plugin returned an invalid or unauthorized opacity change"));
        }
        let node_id = allowed.get(&op.id).copied().ok_or_else(|| bad(command_id, "plugin node ID is invalid"))?;
        if s.doc()?.node(node_id).is_none() {
            return Err(bad(command_id, "plugin's selected node no longer exists"));
        }
    }
    let notices = s.grouped(&format!("Figma plugin: {id}"), |s| {
        for op in &reply.operations {
            let node_id = allowed.get(&op.id).ok_or_else(|| bad(command_id, "plugin node ID is invalid"))?;
            s.execute("node.set", &json!({"id": node_id, "props": {"opacity": op.opacity}}))?;
        }
        apply_edits(s, &reply.edits, allowed, command_id)
    })?;
    Ok(json!({
        "status": if reply.closed { "closed" } else { "open" },
        "id": id,
        "show": reply.show,
        "messages": reply.messages,
        "operations": reply.operations.len() + reply.edits.len(),
        "notifications": notices,
        "closed": reply.closed,
        "message": reply.message,
    }))
}

#[cfg(target_arch = "wasm32")]
fn run(_s: &mut Session, _params: &Value) -> Result<Value> {
    Err(crate::EngineError::Disabled("plugin.figma.run".into(), "the Figma JavaScript worker is not available in the browser yet".into()))
}

#[cfg(not(target_arch = "wasm32"))]
fn run(s: &mut Session, params: &Value) -> Result<Value> {
    run_with(s, params, run_child)
}

#[cfg(not(target_arch = "wasm32"))]
fn run_with(s: &mut Session, params: &Value, execute: impl FnOnce(Request) -> std::result::Result<Response, String>) -> Result<Value> {
    let command_id = "plugin.figma.run";
    let id = string(params, "id").ok_or_else(|| bad(command_id, "`id` required"))?;
    let entry = s.prefs.figma_plugins.iter().find(|p| p.id == id).ok_or_else(|| bad(command_id, "no such local Figma plugin"))?.clone();
    let report = inspect(s, &json!({"path": entry.path}))?;
    if report["digest"].as_str() != Some(entry.digest.as_str()) {
        return Err(bad(command_id, "plugin files changed since import; inspect and import the package again"));
    }
    let manifest_path = PathBuf::from(&entry.path);
    let bytes = file_access::read_capped(&manifest_path, MAX_MANIFEST_BYTES)?;
    let parsed = ParsedManifest::parse(&bytes).map_err(|e| bad(command_id, e))?;
    if parsed.manifest.ui.is_some()
        || !parsed.manifest.editor_type.iter().any(|v| v == "figma")
        || parsed.manifest.document_access.as_deref() != Some("dynamic-page")
    {
        return Err(bad(command_id, "this runtime requires a Figma Design plugin without HTML UI and with dynamic-page access"));
    }
    if parsed.manifest.api.split('.').next() != Some("1") {
        return Err(bad(command_id, "this plugin requires an unsupported API version"));
    }
    let menu_command = string(params, "command");
    if let Some(command) = menu_command.as_deref()
        && !parsed.commands.iter().any(|item| item.command == command)
        && !(command == "textreview" && parsed.manifest.capabilities.iter().any(|v| v == "textreview"))
    {
        return Err(bad(command_id, "unknown plugin menu command"));
    }
    let root = manifest_path.parent().ok_or_else(|| bad(command_id, "manifest has no parent folder"))?;
    let main_path = checked_file(root, &parsed.manifest.main)?;
    let source = String::from_utf8(file_access::read_capped(&main_path, (worker::MAX_REQUEST_BYTES / 2) as u64)?)
        .map_err(|_| bad(command_id, "main JavaScript must be UTF-8"))?;
    // Recheck the package after reading main: a development source must not silently switch code.
    if inspect(s, &json!({"path": entry.path}))?["digest"].as_str() != Some(entry.digest.as_str()) {
        return Err(bad(command_id, "plugin files changed while preparing execution"));
    }
    let (selected, extra) = snapshot(s, params)?;
    let request = Request { source, command: menu_command, selection: selected.clone(), extra };
    let response = execute(request).map_err(|e| bad(command_id, e))?;
    if params.get("parameterInput").is_some() && (!response.operations.is_empty() || !response.edits.is_empty()) {
        return Err(bad(command_id, "parameter suggestions may not edit the document"));
    }
    if !response.closed && response.event_result.is_none() {
        return Err(bad(command_id, "plugin did not call figma.closePlugin()"));
    }
    if response.operations.len() + response.edits.len() > worker::MAX_OPERATIONS {
        return Err(bad(command_id, "plugin operation limit exceeded"));
    }
    let allowed: std::collections::BTreeMap<_, _> = selected.iter().map(|node| (node.id.as_str(), node.id.parse::<figcraft_doc::NodeId>())).collect();
    for op in &response.operations {
        if !op.opacity.is_finite() || !(0.0..=1.0).contains(&op.opacity) || !allowed.contains_key(op.id.as_str()) {
            return Err(bad(command_id, "plugin returned an invalid or unauthorized opacity change"));
        }
    }
    let count = response.operations.len() + response.edits.len();
    let notices = s.grouped(&format!("Figma plugin: {}", entry.name), |s| {
        for op in &response.operations {
            let node_id =
                allowed.get(op.id.as_str()).and_then(|result| result.as_ref().ok()).ok_or_else(|| bad(command_id, "plugin node ID is invalid"))?;
            s.execute("node.set", &json!({"id": node_id, "props": {"opacity": op.opacity}}))?;
        }
        apply_edits(
            s,
            &response.edits,
            &allowed.iter().filter_map(|(key, id)| id.as_ref().ok().map(|id| ((*key).to_string(), *id))).collect(),
            command_id,
        )
    })?;
    Ok(json!({"status": "executed", "id": id, "operations": count, "closed": response.closed,
        "message": response.message, "notifications": notices, "eventResult":response.event_result}))
}

#[cfg(not(target_arch = "wasm32"))]
fn run_child(request: Request) -> std::result::Result<Response, String> {
    let input = serde_json::to_vec(&request).map_err(|e| format!("cannot encode plugin request: {e}"))?;
    if input.len() > worker::MAX_REQUEST_BYTES {
        return Err("plugin request is too large".into());
    }
    let executable = std::env::current_exe().map_err(|e| format!("cannot locate plugin worker: {e}"))?;
    let mut child = Command::new(executable)
        .arg(worker::WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start plugin worker: {e}"))?;
    let stdout = child.stdout.take().ok_or("plugin worker produced no output")?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.take((worker::MAX_RESPONSE_BYTES + 1) as u64).read_to_end(&mut output).map_err(|e| format!("cannot read plugin result: {e}"))?;
        Ok::<Vec<u8>, String>(output)
    });
    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(&input)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("cannot send plugin request: {e}"));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err("plugin worker failed or exceeded its runtime limits".into());
                }
                let output = reader.join().map_err(|_| "plugin worker output reader failed".to_string())??;
                if output.len() > worker::MAX_RESPONSE_BYTES {
                    return Err("plugin response is too large".into());
                }
                return serde_json::from_slice::<std::result::Result<Response, String>>(&output)
                    .map_err(|e| format!("invalid plugin worker reply: {e}"))?;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("plugin exceeded the 2-second time limit".into());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot monitor plugin worker: {e}"));
            }
        }
    }
}

fn reject_link(path: &Path) -> Result<()> {
    if file_access::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(bad(COMMAND, format!("symbolic links are not allowed: {}", path.display())));
    }
    Ok(())
}

fn checked_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        reject_link(&path)?;
    }
    if !file_access::metadata(&path)?.is_file() {
        return Err(bad(COMMAND, format!("not a file: {}", path.display())));
    }
    Ok(path)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            COMMAND,
            "Inspect local Figma plugin",
            "{path: manifest.json or its folder} → manifest, referenced files, SHA-256 digest and compatibility diagnostics; never runs code",
            false,
            inspect
        ),
        spec!(
            "plugin.figma.import",
            "Register local Figma plugin",
            "{path, mode: development} → diagnostic report; saves source and digest, but does not run code",
            false,
            import
        ),
        spec!("plugin.figma.status", "Inspect registered Figma plugin", "{id} → current report and changedSinceImport", false, status),
        spec!("plugin.figma.remove", "Remove registered Figma plugin", "{id} → {removed}", false, remove),
        spec!(
            "plugin.figma.run",
            "Run local Figma plugin",
            "{id, command?, parameters?, text?} runs the bounded native Figma API subset; text is used by textreview",
            false,
            run
        ),
        spec!("plugin.figma.ui.start", "Open local Figma plugin UI", "{id, command?} → HTML UI and initial messages", false, ui_start),
        spec!(
            "plugin.figma.ui.message",
            "Send message to local Figma plugin UI",
            "{id, payload} → edits, messages and close state",
            false,
            ui_message
        ),
        spec!("plugin.figma.ui.close", "Close local Figma plugin UI", "{} → {closed}", false, ui_close),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_package_inspects_without_running_build() {
        let dir = std::env::temp_dir().join(format!("figcraft-figma-plugin-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist/code.js"), "figma.closePlugin()").unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"id":"test.local","name":"Test","api":"1.0.0","editorType":["figma"],"main":"dist/code.js","documentAccess":"dynamic-page","build":"touch should-not-exist"}"#).unwrap();
        let mut session = Session::default();
        let report = session.execute("plugin.figma.inspect", &json!({"path": dir})).unwrap();
        assert_eq!(report["status"], "diagnosticado");
        assert_eq!(report["executable"], true);
        assert_eq!(report["files"][0]["bytes"], 19);
        assert!(!dir.join("should-not-exist").exists());
        let registered = import(&mut session, &json!({"path": dir, "mode": "development"})).unwrap();
        assert_eq!(registered["registered"], true);
        assert!(session.plugins.unsaved);
        assert_eq!(session.prefs.figma_plugins.len(), 1);
        let saved_prefs = serde_json::to_string(&session.prefs).unwrap();
        let loaded_prefs: crate::Prefs = serde_json::from_str(&saved_prefs).unwrap();
        assert_eq!(loaded_prefs.figma_plugins, session.prefs.figma_plugins);
        let first_status = status(&mut session, &json!({"id": "test.local"})).unwrap();
        assert_eq!(first_status["changedSinceImport"], false);
        std::fs::write(dir.join("dist/code.js"), "figma.closePlugin('changed')").unwrap();
        assert_eq!(status(&mut session, &json!({"id": "test.local"})).unwrap()["changedSinceImport"], true);
        assert!(run(&mut session, &json!({"id": "test.local"})).is_err());
        assert_eq!(remove(&mut session, &json!({"id": "test.local"})).unwrap()["removed"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_in_package_is_refused() {
        let dir = std::env::temp_dir().join(format!("figcraft-figma-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.join("code.js")).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"id":"test.link","name":"Test","api":"1.0.0","editorType":["figma"],"main":"code.js"}"#)
            .unwrap();
        let mut session = Session::default();
        assert!(inspect(&mut session, &json!({"path": dir})).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn created_plugin_layers_keep_hierarchy_and_undo_together() {
        let mut session = Session::default();
        let edits = vec![
            Edit::CreateNode { id: "plugin:1".into(), node_type: "frame".into() },
            Edit::Resize { id: "plugin:1".into(), width: 800.0, height: 600.0 },
            Edit::AppendChild { parent: "page".into(), id: "plugin:1".into(), x: 50.0, y: 60.0 },
            Edit::CreateNode { id: "plugin:2".into(), node_type: "rectangle".into() },
            Edit::AppendChild { parent: "plugin:1".into(), id: "plugin:2".into(), x: 20.0, y: 30.0 },
            Edit::SetNode { id: "plugin:2".into(), key: "fills".into(), value: json!([{"type":"SOLID","color":{"r":1.0,"g":0.0,"b":0.0}}]) },
            Edit::Select { ids: vec!["plugin:2".into()] },
        ];
        let before = session.undo_depth().0;
        session.grouped("Plugin layers", |s| apply_edits(s, &edits, &Default::default(), "test")).unwrap();
        let rect = session.selection()[0];
        let frame = session.doc().unwrap().node(rect).unwrap().parent.unwrap();
        assert_eq!(session.doc().unwrap().node(frame).unwrap().parent, Some(session.page().unwrap()));
        assert_eq!(session.doc().unwrap().node(frame).unwrap().width, 800.0);
        assert_eq!(session.doc().unwrap().node(rect).unwrap().name, "Rectangle");
        assert_eq!(session.undo_depth().0, before + 1);
        session.execute("edit.undo", &json!({})).unwrap();
        assert!(session.doc().unwrap().node(rect).is_none());
        assert!(session.doc().unwrap().node(frame).is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn plugin_write_is_one_undo_step_and_failed_script_keeps_document() {
        let dir = std::env::temp_dir().join(format!("figcraft-figma-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = r#"{"id":"test.run","name":"Opacity","api":"1.0.0","editorType":["figma"],"main":"code.js","documentAccess":"dynamic-page"}"#;
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(dir.join("code.js"), "figma.currentPage.selection[0].opacity = 0.4; figma.closePlugin()").unwrap();
        let mut session = Session::default();
        let created = session.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
        let node_id = figcraft_doc::NodeId(created["id"].as_u64().unwrap());
        session.set_selection(vec![node_id]);
        import(&mut session, &json!({"path": dir, "mode": "development"})).unwrap();
        let before = session.undo_depth().0;
        let result = run_with(&mut session, &json!({"id": "test.run"}), worker::execute).unwrap();
        assert_eq!(result["operations"], 1);
        assert!((session.doc().unwrap().node(node_id).unwrap().opacity - 0.4).abs() < 0.0001);
        assert_eq!(session.undo_depth().0, before + 1);
        session.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(session.doc().unwrap().node(node_id).unwrap().opacity, 1.0);

        std::fs::write(dir.join("code.js"), "figma.currentPage.selection[0].opacity = 0.2; throw Error('failed')").unwrap();
        assert!(run_with(&mut session, &json!({"id": "test.run"}), worker::execute).is_err());
        assert_eq!(session.doc().unwrap().node(node_id).unwrap().opacity, 1.0);
        import(&mut session, &json!({"path": dir, "mode": "development"})).unwrap();
        assert!(run_with(&mut session, &json!({"id": "test.run"}), worker::execute).is_err());
        assert_eq!(session.doc().unwrap().node(node_id).unwrap().opacity, 1.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn ui_message_applies_one_undo_step_and_rejects_foreign_nodes() {
        let mut session = Session::default();
        let created = session.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
        let node_id = figcraft_doc::NodeId(created["id"].as_u64().unwrap());
        let allowed = std::collections::BTreeMap::from([(node_id.to_string(), node_id)]);
        let before = session.undo_depth().0;
        let reply = UiResponse {
            show: None,
            messages: vec![json!({"done": true})],
            operations: vec![worker::SetOpacity { id: node_id.to_string(), opacity: 0.25 }],
            edits: Vec::new(),
            closed: false,
            message: None,
            event_result: None,
        };
        let result = apply_ui_operations(&mut session, "test.ui", &allowed, reply, "plugin.figma.ui.message").unwrap();
        assert_eq!(result["operations"], 1);
        assert_eq!(session.undo_depth().0, before + 1);
        assert_eq!(session.doc().unwrap().node(node_id).unwrap().opacity, 0.25);
        session.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(session.doc().unwrap().node(node_id).unwrap().opacity, 1.0);

        let bad_reply = UiResponse {
            show: None,
            messages: Vec::new(),
            operations: vec![worker::SetOpacity { id: "999999".into(), opacity: 0.5 }],
            edits: Vec::new(),
            closed: false,
            message: None,
            event_result: None,
        };
        assert!(apply_ui_operations(&mut session, "test.ui", &allowed, bad_reply, "plugin.figma.ui.message").is_err());
        assert_eq!(session.doc().unwrap().node(node_id).unwrap().opacity, 1.0);
    }
}
