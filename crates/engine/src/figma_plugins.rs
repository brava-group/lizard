//! Local Figma JavaScript plugin diagnosis and a small, bounded execution slice. This is separate
//! from `plugin.import`, which only accepts FigCraft's metered WebAssembly plugins.

#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Command, Stdio};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use figcraft_figma_plugin::worker::{self, Request, Response, SelectedNode};
use figcraft_figma_plugin::{MANIFEST_FILE, MAX_FILE_BYTES, MAX_MANIFEST_BYTES, MAX_PACKAGE_BYTES, ParsedManifest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cmd::CommandSpec;
use crate::params::string;
use crate::{Result, Session, bad, file_access, spec};

const COMMAND: &str = "plugin.figma.inspect";

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
    diagnostics.push("Execution supports only selection name/opacity reads, opacity writes and closePlugin; inspect never runs JavaScript".into());
    Ok(json!({
        "status": "diagnosticado",
        "executable": false,
        "name": parsed.manifest.name,
        "id": parsed.manifest.id,
        "api": parsed.manifest.api,
        "editorType": parsed.manifest.editor_type,
        "documentAccess": parsed.manifest.document_access,
        "networkAccess": parsed.manifest.network_access,
        "commands": parsed.commands,
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
    let before = s.prefs.figma_plugins.len();
    s.prefs.figma_plugins.retain(|p| p.id != id);
    let removed = before != s.prefs.figma_plugins.len();
    s.plugins.unsaved |= removed;
    Ok(json!({"removed": removed, "id": id}))
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
    if parsed.manifest.api.split('.').next() != Some("1")
        || parsed.manifest.network_access.as_ref().is_some_and(|network| network.allowed_domains.iter().any(|v| v != "none"))
        || !parsed.manifest.parameters.is_empty()
    {
        return Err(bad(command_id, "this plugin requires an unsupported API version, network access or parameters"));
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
    // Recheck the package after reading main: a development source must not silently switch code.
    if inspect(s, &json!({"path": entry.path}))?["digest"].as_str() != Some(entry.digest.as_str()) {
        return Err(bad(command_id, "plugin files changed while preparing execution"));
    }
    let selected: Vec<SelectedNode> = s
        .selection()
        .into_iter()
        .filter_map(|node_id| {
            s.doc().ok()?.node(node_id).map(|node| SelectedNode { id: node_id.to_string(), name: node.name.clone(), opacity: node.opacity })
        })
        .collect();
    let request = Request { source, command: menu_command, selection: selected.clone() };
    let response = execute(request).map_err(|e| bad(command_id, e))?;
    if !response.closed {
        return Err(bad(command_id, "plugin did not call figma.closePlugin()"));
    }
    if response.operations.len() > worker::MAX_OPERATIONS {
        return Err(bad(command_id, "plugin operation limit exceeded"));
    }
    let allowed: std::collections::BTreeMap<_, _> = selected.iter().map(|node| (node.id.as_str(), node.id.parse::<figcraft_doc::NodeId>())).collect();
    for op in &response.operations {
        if !op.opacity.is_finite() || !(0.0..=1.0).contains(&op.opacity) || !allowed.contains_key(op.id.as_str()) {
            return Err(bad(command_id, "plugin returned an invalid or unauthorized opacity change"));
        }
    }
    let count = response.operations.len();
    s.grouped(&format!("Figma plugin: {}", entry.name), |s| {
        for op in &response.operations {
            let node_id =
                allowed.get(op.id.as_str()).and_then(|result| result.as_ref().ok()).ok_or_else(|| bad(command_id, "plugin node ID is invalid"))?;
            s.execute("node.set", &json!({"id": node_id, "props": {"opacity": op.opacity}}))?;
        }
        Ok(())
    })?;
    Ok(json!({"status": "executed", "id": id, "operations": count, "closed": true, "message": response.message}))
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
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(&input) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("cannot send plugin request: {e}"));
        }
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
            "{id, command?} runs the synchronous selection/opacity subset in a bounded native worker",
            false,
            run
        ),
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
        assert_eq!(report["executable"], false);
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
}
