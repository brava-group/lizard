//! Parse a local Figma plugin manifest and run a bounded subset of its JavaScript API.
//! The editor owns file I/O and applies validated document operations after worker completion.

#[cfg(not(target_arch = "wasm32"))]
pub mod bridge;
pub mod catalog;
#[cfg(not(target_arch = "wasm32"))]
pub mod worker;
#[cfg(not(target_arch = "wasm32"))]
pub mod worker_ui;

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MANIFEST_FILE: &str = "manifest.json";
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_FILES: usize = 65;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UiFiles {
    One(String),
    Named(BTreeMap<String, String>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub api: String,
    pub main: String,
    #[serde(default)]
    pub editor_type: Vec<String>,
    #[serde(default)]
    pub ui: Option<UiFiles>,
    #[serde(default)]
    pub menu: Vec<Value>,
    #[serde(default)]
    pub network_access: Option<NetworkAccess>,
    #[serde(default)]
    pub document_access: Option<String>,
    #[serde(default)]
    pub parameters: Vec<Value>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub build: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAccess {
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub dev_allowed_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MenuCommand {
    pub label: String,
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ParsedManifest {
    pub manifest: Manifest,
    pub files: Vec<String>,
    pub commands: Vec<MenuCommand>,
    pub diagnostics: Vec<String>,
}

impl ParsedManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("manifest.json is too large".into());
        }
        let manifest: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("invalid manifest.json: {e}"))?;
        if manifest.id.trim().is_empty() || manifest.id.len() > 128 || manifest.id.chars().any(char::is_control) {
            return Err("`id` must be 1–128 printable characters".into());
        }
        if manifest.name.trim().is_empty() || manifest.name.len() > 128 || manifest.name.chars().any(char::is_control) {
            return Err("`name` must be 1–128 printable characters".into());
        }
        if manifest.api.len() > 32 || !manifest.api.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())) {
            return Err("`api` must be a dotted numeric version".into());
        }
        if manifest.editor_type.is_empty()
            || manifest.editor_type.len() > 5
            || manifest.editor_type.iter().any(|v| !matches!(v.as_str(), "figma" | "figjam" | "dev" | "slides" | "buzz"))
        {
            return Err("`editorType` must list known Figma editors".into());
        }
        if manifest.editor_type.iter().any(|v| v == "figjam") && manifest.editor_type.iter().any(|v| v == "dev") {
            return Err("`editorType` cannot combine figjam and dev".into());
        }
        if manifest.document_access.as_deref().is_some_and(|v| v != "dynamic-page") {
            return Err("`documentAccess` must be dynamic-page when present".into());
        }
        let mut files = vec![manifest.main.clone()];
        match &manifest.ui {
            Some(UiFiles::One(path)) => files.push(path.clone()),
            Some(UiFiles::Named(named)) => {
                if named.is_empty() {
                    return Err("`ui` map is empty".into());
                }
                for (key, path) in named {
                    if key.is_empty() || key.len() > 64 || key.chars().any(char::is_control) {
                        return Err("`ui` has an invalid key".into());
                    }
                    files.push(path.clone());
                }
            }
            None => {}
        }
        if files.len() > MAX_FILES {
            return Err("too many referenced files".into());
        }
        for path in &files {
            validate_relative_path(path)?;
        }
        let mut commands = Vec::new();
        parse_menu(&manifest.menu, "", 0, &mut commands)?;
        if manifest.parameters.len() > 64 {
            return Err("too many parameters".into());
        }
        let mut diagnostics = Vec::new();
        if !manifest.editor_type.iter().any(|v| v == "figma") {
            diagnostics.push("This plugin does not target the Figma design editor".into());
        }
        if manifest.ui.is_some() {
            diagnostics.push("HTML UI preview supports local, self-contained files on macOS; browser and other desktop platforms are pending".into());
        }
        if manifest.network_access.as_ref().is_some_and(|v| !v.allowed_domains.is_empty() && v.allowed_domains != ["none"]) {
            diagnostics
                .push("HTML UI preview loads stylesheets and fonts from declared HTTPS hosts; other network requests are not supported yet".into());
        }
        if manifest.build.is_some() {
            diagnostics.push("The manifest's build command was not run; provide compiled main JavaScript".into());
        }
        if !manifest.parameters.is_empty() {
            diagnostics.push("Quick action parameters are supported for local plugins; unsupported parameter types may be ignored".into());
        }
        if manifest.document_access.is_none() {
            diagnostics.push("Legacy full-document loading is not supported yet".into());
        }
        if manifest.api.split('.').next() != Some("1") {
            diagnostics.push("This Figma API version has not been validated".into());
        }
        if let Some(network) = &manifest.network_access
            && (network.allowed_domains.is_empty()
                || network.allowed_domains.len() > 64
                || network.dev_allowed_domains.len() > 64
                || network
                    .allowed_domains
                    .iter()
                    .chain(&network.dev_allowed_domains)
                    .any(|v| v.is_empty() || v.len() > 255 || v.chars().any(char::is_control)))
        {
            return Err("`networkAccess` has invalid or excessive domains".into());
        }
        Ok(Self { manifest, files, commands, diagnostics })
    }
}

/// Reject escapes on every platform, including Windows paths while running on Unix.
pub fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 1024
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        || Path::new(path).components().any(|part| !matches!(part, Component::Normal(_)))
        || path.contains(':')
    {
        return Err(format!("unsafe plugin file path: {path}"));
    }
    Ok(())
}

fn parse_menu(items: &[Value], prefix: &str, depth: usize, out: &mut Vec<MenuCommand>) -> Result<(), String> {
    if depth > 8 || items.len() > 128 || out.len().saturating_add(items.len()) > 128 {
        return Err("plugin menu is too deep or large".into());
    }
    for item in items {
        let obj = item.as_object().ok_or("menu item must be an object")?;
        if obj.get("separator") == Some(&Value::Bool(true)) && obj.len() == 1 {
            continue;
        }
        let name = obj.get("name").and_then(Value::as_str).ok_or("menu item needs a name")?;
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err("invalid menu name".into());
        }
        let label = if prefix.is_empty() { name.to_string() } else { format!("{prefix} / {name}") };
        match (obj.get("command").and_then(Value::as_str), obj.get("menu")) {
            (Some(command), None) if !command.is_empty() && command.len() <= 128 && !command.chars().any(char::is_control) => {
                let parameters = obj.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
                if parameters.len() > 32 || parameters.iter().any(|v| v["key"].as_str().is_none_or(|key| key.is_empty() || key.len() > 64)) {
                    return Err("plugin menu has invalid parameters".into());
                }
                out.push(MenuCommand { label, command: command.to_string(), parameters });
            }
            (None, Some(Value::Array(children))) => parse_menu(children, &label, depth + 1, out)?,
            _ => return Err("menu item must be a command, submenu or separator".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = r#"{"name":"Shapes","id":"local.shapes","api":"1.0.0","editorType":["figma"],"main":"dist/code.js","documentAccess":"dynamic-page","menu":[{"name":"Create","menu":[{"name":"Rectangle","command":"rect"}]}]}"#;

    #[test]
    fn valid_manifest_and_submenu() {
        let parsed = ParsedManifest::parse(BASIC.as_bytes()).unwrap();
        assert_eq!(parsed.files, ["dist/code.js"]);
        assert_eq!(parsed.commands[0].command, "rect");
        assert_eq!(parsed.commands[0].label, "Create / Rectangle");
    }

    #[test]
    fn rejects_escape_and_invalid_menu() {
        for path in ["../secret", "/etc/passwd", "C:\\secret", "a/../b", "a//b", "a\\b"] {
            assert!(validate_relative_path(path).is_err(), "{path}");
        }
        let broken = BASIC.replace("dist/code.js", "../secret");
        assert!(ParsedManifest::parse(broken.as_bytes()).is_err());
        let broken = BASIC.replace("\"command\":\"rect\"", "\"menu\":{} ");
        assert!(ParsedManifest::parse(broken.as_bytes()).is_err());
    }

    #[test]
    fn ui_and_build_are_diagnostic_only() {
        let input = BASIC.replace("\"documentAccess\"", "\"ui\":\"ui.html\",\"build\":\"touch marker\",\"documentAccess\"");
        let parsed = ParsedManifest::parse(input.as_bytes()).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert!(parsed.diagnostics.iter().any(|v| v.contains("build command")));
        assert!(parsed.diagnostics.iter().any(|v| v.contains("HTML UI preview")));
    }
}
