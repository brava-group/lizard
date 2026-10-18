//! Read-only discovery of Figma Community plugins. The search endpoint is internal and may change;
//! catalogue data is never treated as a package or executable code.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PAGE_SIZE: usize = 20;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub resource_id: String,
    pub plugin_id: String,
    pub name: String,
    pub tagline: String,
    pub version_id: Option<String>,
    pub page_url: String,
    /// `None` means the response did not include the current version's manifest.
    pub has_ui: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub total: usize,
    pub from: usize,
    pub entries: Vec<Entry>,
}

/// Build only the fixed read-only endpoint. Bound the query before percent encoding it.
pub fn search_url(query: &str, from: usize) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 50 || query.chars().any(char::is_control) {
        return Err("search requires 1–50 printable characters".into());
    }
    if from > 10_000 {
        return Err("catalogue pagination limit reached".into());
    }
    let mut encoded = String::new();
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    Ok(format!(
        "https://www.figma.com/api/search/resources?query={encoded}&resource_type=plugin&sort_by=relevancy&price=all&resource_editor_type=design&include_content=true&from={}&page_size={PAGE_SIZE}",
        from
    ))
}

/// Normalize untrusted catalogue JSON. Missing or malformed items are skipped.
pub fn parse_page(bytes: &[u8], from: usize) -> Result<Page, String> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("catalogue response is too large".into());
    }
    let root: Value = serde_json::from_slice(bytes).map_err(|_| "catalogue did not return JSON".to_string())?;
    if root["error"] == true || root["status"].as_u64().is_some_and(|status| status != 200) {
        return Err("catalogue refused the request".into());
    }
    let meta = root.get("meta").ok_or_else(|| "catalogue response has no results".to_string())?;
    let total = meta["total_hits"].as_u64().ok_or_else(|| "catalogue response has no result count".to_string())?;
    let results = meta["results"].as_array().ok_or_else(|| "catalogue response has no result list".to_string())?;
    if results.len() > 100 {
        return Err("catalogue sent too many results".into());
    }
    let mut entries = Vec::new();
    for result in results {
        let model = &result["model"];
        if model["resource_type"] != "plugin" {
            continue;
        }
        let (Some(resource_id), Some(plugin_id), Some(name)) = (model["id"].as_str(), model["content_id"].as_str(), model["name"].as_str()) else {
            continue;
        };
        if resource_id.len() > 128
            || plugin_id.is_empty()
            || plugin_id.len() > 32
            || !plugin_id.bytes().all(|b| b.is_ascii_digit())
            || name.is_empty()
            || name.len() > 256
        {
            continue;
        }
        let plugin = &model["content"]["plugin"];
        let version_id = plugin["current_plugin_version_id"].as_str().filter(|v| v.len() <= 32 && v.bytes().all(|b| b.is_ascii_digit()));
        let has_ui = version_id
            .and_then(|id| plugin["versions"].get(id))
            .and_then(|version| version["manifest"].as_object())
            .map(|manifest| manifest.get("ui").is_some_and(|ui| !ui.is_null()));
        let tagline = model["tagline"].as_str().filter(|v| v.len() <= 1000).unwrap_or_default().to_string();
        entries.push(Entry {
            resource_id: resource_id.to_string(),
            plugin_id: plugin_id.to_string(),
            name: name.to_string(),
            tagline,
            version_id: version_id.map(str::to_string),
            page_url: format!("https://www.figma.com/community/plugin/{plugin_id}"),
            has_ui,
        });
    }
    Ok(Page { total: usize::try_from(total).unwrap_or(usize::MAX), from, entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_search_without_accepting_an_endpoint() {
        let url = search_url("ícones & UI", 40).unwrap();
        assert!(url.contains("query=%C3%ADcones%20%26%20UI"));
        assert!(url.contains("from=40&page_size=20"));
        assert!(search_url(" ", 0).is_err());
        assert!(search_url(&"a".repeat(51), 0).is_err());
    }

    #[test]
    fn separates_resource_id_from_plugin_id() {
        let json = br#"{"status":200,"error":false,"meta":{"total_hits":2,"results":[{"model":{"resource_type":"plugin","id":"resource-uuid","content_id":"12345","name":"Example","tagline":"Makes shapes","content":{"plugin":{"current_plugin_version_id":"7","versions":{"7":{"manifest":{"ui":"ui.html"}}}}}}},{"model":{"resource_type":"widget","id":"other","content_id":"3","name":"Skip"}}]}}"#;
        let page = parse_page(json, 0).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.entries[0].resource_id, "resource-uuid");
        assert_eq!(page.entries[0].plugin_id, "12345");
        assert_eq!(page.entries[0].has_ui, Some(true));
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].page_url, "https://www.figma.com/community/plugin/12345");
    }

    #[test]
    fn rejects_invalid_responses() {
        assert!(parse_page(&vec![b'a'; MAX_RESPONSE_BYTES + 1], 0).is_err());
        assert!(parse_page(br#"{"status":403,"error":true}"#, 0).is_err());
        assert!(parse_page(br#"{"meta":{"total_hits":0}}"#, 0).is_err());
        let no_content = br#"{"status":200,"error":false,"meta":{"total_hits":1,"results":[{"model":{"resource_type":"plugin","id":"r","content_id":"5","name":"Unavailable"}}]}}"#;
        assert_eq!(parse_page(no_content, 0).unwrap().entries[0].has_ui, None);
    }
}
