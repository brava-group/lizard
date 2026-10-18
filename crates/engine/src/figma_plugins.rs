//! Read-only diagnosis of a local Figma JavaScript plugin package. It is deliberately separate
//! from `plugin.import`, which only accepts FigCraft's metered WebAssembly plugins.

use std::path::{Path, PathBuf};

use figcraft_figma_plugin::{MANIFEST_FILE, MAX_FILE_BYTES, MAX_MANIFEST_BYTES, MAX_PACKAGE_BYTES, ParsedManifest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cmd::CommandSpec;
use crate::params::string;
use crate::{Result, Session, bad, file_access, spec};

const COMMAND: &str = "plugin.figma.inspect";

/// A development source recorded in preferences. It stays diagnostic only until a JS runtime is
/// implemented and tested. No package bytes or JavaScript are executed when it is loaded.
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
    diagnostics.push("JavaScript execution is not implemented yet; this report does not run the plugin".into());
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

fn run(_s: &mut Session, _params: &Value) -> Result<Value> {
    Err(crate::EngineError::Disabled("plugin.figma.run".into(), "Figma JavaScript plugin execution has not been implemented or validated".into()))
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
        spec!("plugin.figma.run", "Run Figma plugin", "{id, command?} unavailable until a validated JavaScript runtime exists", false, run),
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
}
