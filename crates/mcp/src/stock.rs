//! Unsplash photo search and placement for the in-app agent. The access key stays in the local
//! process environment; neither tool results nor document metadata contain it.

use serde_json::{Map, Value, json};

use crate::backend::Backend;
use crate::tools::ToolResult;

const KEY_ENV: &str = "LIZARD_UNSPLASH_ACCESS_KEY";
const REFERRAL: &str = "utm_source=lizard&utm_medium=referral";

fn key() -> Result<String, String> {
    std::env::var(KEY_ENV)
        .ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| format!("Unsplash is not configured. Set {KEY_ENV} in the Lizard app environment and restart it."))
}

fn required<'a>(args: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    args.get(name).and_then(Value::as_str).filter(|value| !value.trim().is_empty()).ok_or_else(|| format!("`{name}` must be a non-empty string"))
}

fn referral(url: &str) -> String {
    format!("{url}{}{REFERRAL}", if url.contains('?') { '&' } else { '?' })
}

fn photo_summary(photo: &Value) -> Value {
    let profile = photo.pointer("/user/links/html").and_then(Value::as_str).unwrap_or("");
    let page = photo.pointer("/links/html").and_then(Value::as_str).unwrap_or("");
    json!({
        "id": photo["id"],
        "description": photo["alt_description"],
        "width": photo["width"], "height": photo["height"],
        "previewUrl": photo.pointer("/urls/small"),
        "imageUrl": photo.pointer("/urls/regular"),
        "photographer": photo.pointer("/user/name"),
        "photographerUrl": referral(profile),
        "photoUrl": referral(page),
        "source": "Unsplash",
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .http_status_as_error(false)
        .max_redirects(2)
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .into()
}

#[cfg(not(target_arch = "wasm32"))]
fn api_get(url: &str, access_key: &str) -> Result<Value, String> {
    let mut response = agent()
        .get(url)
        .header("Authorization", format!("Client-ID {access_key}"))
        .header("Accept-Version", "v1")
        .call()
        .map_err(|error| format!("Unsplash request failed: {error}"))?;
    let status = response.status().as_u16();
    let bytes =
        response.body_mut().with_config().limit(2 * 1024 * 1024).read_to_vec().map_err(|error| format!("Unsplash response failed: {error}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| format!("Invalid Unsplash response: {error}"))?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "Unsplash returned HTTP {status}: {}",
            value["errors"].as_array().and_then(|errors| errors.first()).and_then(Value::as_str).unwrap_or("request failed")
        ));
    }
    Ok(value)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn search(backend: &mut dyn Backend, args: &Map<String, Value>) -> Result<ToolResult, String> {
    let result = if backend.has_ui() { backend.call("stock.search", Value::Object(args.clone()))? } else { search_api(args)? };
    Ok(ToolResult::json(&result))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn search_api(args: &Map<String, Value>) -> Result<Value, String> {
    let query = required(args, "query")?.trim();
    if query.len() > 120 {
        return Err("`query` is too long".into());
    }
    let access_key = key()?;
    let mut request = agent()
        .get("https://api.unsplash.com/search/photos")
        .header("Authorization", format!("Client-ID {access_key}"))
        .header("Accept-Version", "v1")
        .query("query", query)
        .query("per_page", "8");
    if let Some(orientation) = args.get("orientation").and_then(Value::as_str) {
        if !matches!(orientation, "landscape" | "portrait" | "squarish") {
            return Err("`orientation` must be landscape, portrait or squarish".into());
        }
        request = request.query("orientation", orientation);
    }
    let mut response = request.call().map_err(|error| format!("Unsplash search failed: {error}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .map_err(|error| format!("Unsplash search response failed: {error}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| format!("Invalid Unsplash search response: {error}"))?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "Unsplash returned HTTP {status}: {}",
            value["errors"].as_array().and_then(|errors| errors.first()).and_then(Value::as_str).unwrap_or("request failed")
        ));
    }
    let photos = value["results"].as_array().ok_or("Unsplash search returned no results array")?;
    Ok(json!({"query": query, "photos": photos.iter().map(photo_summary).collect::<Vec<_>>(), "total": value["total"]}))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn fetch_api(photo_id: &str) -> Result<Value, String> {
    use base64::Engine as _;

    if photo_id.len() > 64 || !photo_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) {
        return Err("`photoId` is invalid".into());
    }
    let access_key = key()?;
    let photo = api_get(&format!("https://api.unsplash.com/photos/{photo_id}"), &access_key)?;
    let image_url = photo.pointer("/urls/regular").and_then(Value::as_str).ok_or("Unsplash photo has no image URL")?;
    let download_url = photo.pointer("/links/download_location").and_then(Value::as_str).ok_or("Unsplash photo has no download tracking URL")?;
    if !image_url.starts_with("https://images.unsplash.com/") || !download_url.starts_with("https://api.unsplash.com/photos/") {
        return Err("Unsplash returned an unexpected image or tracking URL".into());
    }
    let mut response = agent().get(image_url).call().map_err(|error| format!("Photo download failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Photo download returned HTTP {}", response.status()));
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .map_err(|error| format!("Photo is too large or could not be read: {error}"))?;
    let tracking = api_get(download_url, &access_key);
    Ok(json!({
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        "photo": photo_summary(&photo),
        "downloadTracked": tracking.is_ok(),
        "trackingError": tracking.err(),
    }))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn place(backend: &mut dyn Backend, args: &Map<String, Value>) -> Result<ToolResult, String> {
    let photo_id = required(args, "photoId")?;
    if photo_id.len() > 64 || !photo_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) {
        return Err("`photoId` is invalid".into());
    }
    let fetched = if backend.has_ui() { backend.call("stock.fetch", json!({"photoId": photo_id}))? } else { fetch_api(photo_id)? };
    let summary = &fetched["photo"];
    let photographer = summary["photographer"].as_str().unwrap_or("Unknown photographer");
    let data = fetched["data"].as_str().ok_or("Unsplash photo has no image data")?;
    let mut params = json!({"data": data, "mode": "crop", "name": format!("Photo by {photographer} · Unsplash")});
    for field in ["target", "parent", "x", "y", "width", "height"] {
        if let Some(value) = args.get(field) {
            params[field] = value.clone();
        }
    }
    let mut metadata = json!({"lizard.stock.source": "Unsplash", "lizard.stock.photo_id": photo_id});
    for (field, key) in
        [("photographer", "photographer"), ("photographerUrl", "photographer_url"), ("photoUrl", "photo_url"), ("imageUrl", "image_url")]
    {
        metadata[format!("lizard.stock.{key}")] = summary[field].clone();
    }
    let node_id = args.get("target").cloned().unwrap_or_else(|| json!({"$ref": "photo"}));
    let result = backend.call(
        "engine.execute",
        json!({
            "command": "command.batch",
            "params": {"label": "Place Unsplash photo", "commands": [
                {"command": "image.place", "as": "photo", "params": params},
                {"command": "node.set", "params": {"ids": [node_id], "props": {"pluginData": metadata}}}
            ]}
        }),
    )?;
    // A placement is a download-like action. Register it only after the document accepts the image.
    Ok(ToolResult::json(
        &json!({"steps": result, "photo": summary, "downloadTracked": fetched["downloadTracked"], "trackingError": fetched["trackingError"]}),
    ))
}

#[cfg(target_arch = "wasm32")]
pub fn search(_backend: &mut dyn Backend, _args: &Map<String, Value>) -> Result<ToolResult, String> {
    Err("Unsplash photos are available in the desktop app".into())
}

#[cfg(target_arch = "wasm32")]
pub fn place(_backend: &mut dyn Backend, _args: &Map<String, Value>) -> Result<ToolResult, String> {
    Err("Unsplash photos are available in the desktop app".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AttachedFake {
        calls: Vec<String>,
    }

    impl Backend for AttachedFake {
        fn call(&mut self, method: &str, _params: Value) -> Result<Value, String> {
            self.calls.push(method.into());
            match method {
                "stock.search" => Ok(json!({"photos": []})),
                "stock.fetch" => Ok(
                    json!({"data": "aGVsbG8=", "photo": {"photographer": "Ada", "photographerUrl": "https://unsplash.com/@ada", "photoUrl": "https://unsplash.com/photos/abc", "imageUrl": "https://images.unsplash.com/example"}, "downloadTracked": true}),
                ),
                "engine.execute" => Ok(json!([{"id": 5}, {"ids": [5]}])),
                _ => Err("unexpected method".into()),
            }
        }

        fn has_ui(&self) -> bool {
            true
        }

        fn describe(&self) -> String {
            "attached fake".into()
        }
    }

    #[test]
    fn summary_has_photographer_and_referral_links() {
        let photo = json!({"id":"abc", "urls":{"regular":"https://images.unsplash.com/example"}, "user":{"name":"Ada","links":{"html":"https://unsplash.com/@ada"}}, "links":{"html":"https://unsplash.com/photos/abc"}});
        let summary = photo_summary(&photo);
        assert_eq!(summary["photographer"], "Ada");
        assert_eq!(summary["photographerUrl"], "https://unsplash.com/@ada?utm_source=lizard&utm_medium=referral");
        assert_eq!(summary["photoUrl"], "https://unsplash.com/photos/abc?utm_source=lizard&utm_medium=referral");
    }

    #[test]
    fn malformed_photo_id_is_rejected_before_network_access() {
        let mut backend = crate::Headless::new();
        let args = Map::from_iter([("photoId".into(), json!("../../unexpected-host"))]);
        let error = place(&mut backend, &args).err().unwrap_or_default();
        assert!(error.contains("invalid"), "{error}");
    }

    #[test]
    fn attached_agent_uses_desktop_bridge_without_a_local_key() {
        let mut backend = AttachedFake { calls: Vec::new() };
        let search_args = Map::from_iter([("query".into(), json!("fern"))]);
        assert!(search(&mut backend, &search_args).is_ok());
        let place_args = Map::from_iter([("photoId".into(), json!("abc"))]);
        let result = place(&mut backend, &place_args);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(backend.calls, ["stock.search", "stock.fetch", "engine.execute"]);
    }
}
