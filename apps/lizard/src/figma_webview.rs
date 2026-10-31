//! macOS child WebView for a local Figma plugin's self-contained HTML.
//! The WebView has no document access: only JSON pluginMessage values reach engine commands.

use std::cell::RefCell;
use std::rc::Rc;

use egui::Rect;
use serde_json::Value;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, WebView, WebViewBuilder};

pub enum Event {
    Ready,
    Message(Value),
}

pub struct Snapshot {
    pub id: String,
    pub html: String,
    pub allowed_domains: Vec<String>,
    pub rect: Option<Rect>,
    pub outgoing: Vec<Value>,
}

pub struct PluginWebView {
    id: String,
    html: String,
    allowed_domains: Vec<String>,
    view: WebView,
    ready: bool,
    pending: Vec<Value>,
}

impl PluginWebView {
    pub fn mark_ready(&mut self) {
        self.ready = true;
    }

    pub fn sync(
        slot: &mut Option<Self>,
        frame: &eframe::Frame,
        ctx: &egui::Context,
        inbox: Rc<RefCell<Vec<Event>>>,
        snapshot: Option<Snapshot>,
    ) -> Result<(), String> {
        let Some(snapshot) = snapshot else {
            *slot = None;
            return Ok(());
        };
        let Some(rect) = snapshot.rect else {
            if let Some(existing) = slot.as_ref() {
                let _ = existing.view.set_visible(false);
            }
            return Ok(());
        };
        let bounds = wry::Rect {
            position: LogicalPosition::new(f64::from(rect.left()), f64::from(rect.top())).into(),
            size: LogicalSize::new(f64::from(rect.width()), f64::from(rect.height())).into(),
        };
        if slot.as_ref().is_none_or(|open| open.id != snapshot.id || open.html != snapshot.html || open.allowed_domains != snapshot.allowed_domains) {
            *slot = None;
            let window = frame.winit_window().ok_or("no native window for the plugin UI")?;
            let ipc_inbox = inbox.clone();
            let ipc_ctx = ctx.clone();
            let network_origins = stylesheet_origins(&snapshot.allowed_domains);
            let policy = format!(
                "default-src 'none'; img-src data: blob: {network_origins}; font-src data: {network_origins}; style-src 'unsafe-inline' {network_origins}; script-src 'unsafe-inline' {network_origins}; connect-src {network_origins}; frame-src 'none'; object-src 'none'; form-action 'none'"
            );
            let html = html_with_policy(&snapshot.html, &policy);
            let view = WebViewBuilder::new()
                .with_bounds(bounds)
                .with_html(html)
                .with_incognito(true)
                .with_clipboard(false)
                .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                .with_navigation_handler(|url| url == "about:blank")
                .with_initialization_script(
                    r#"(() => {
                        const diagnostic = event => {
                            const error = event.reason || event.error || event.message || event;
                            const message = String(error && (error.stack || error.message || error));
                            window.ipc.postMessage(JSON.stringify({ kind: 'diagnostic', message: message.slice(0, 1024) }));
                        };
                        window.addEventListener('error', diagnostic);
                        window.addEventListener('unhandledrejection', diagnostic);
                        window.addEventListener('load', () => setTimeout(() =>
                            window.ipc.postMessage(JSON.stringify({ kind: 'ready' })), 150));
                        const encodeWire = (value, depth = 0) => {
                            if (depth > 24) throw Error('plugin UI message is too deeply nested');
                            if (value instanceof Uint8Array || value instanceof ArrayBuffer) {
                                const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
                                if (bytes.length > 4 * 1024 * 1024) throw Error('plugin binary message is too large');
                                let binary = '';
                                for (let i = 0; i < bytes.length; i += 8192)
                                    binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
                                return { __lizardBytes: btoa(binary) };
                            }
                            if (Array.isArray(value)) return value.map(item => encodeWire(item, depth + 1));
                            if (value && typeof value === 'object')
                                return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, encodeWire(item, depth + 1)]));
                            return value;
                        };
                        window.__lizardReviveWire = (value, depth = 0) => {
                            if (depth > 24) throw Error('plugin UI message is too deeply nested');
                            if (value && typeof value === 'object' && typeof value.__lizardBytes === 'string'
                                && Object.keys(value).length === 1) {
                                const binary = atob(value.__lizardBytes);
                                const bytes = new Uint8Array(binary.length);
                                for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
                                return bytes;
                            }
                            if (Array.isArray(value)) return value.map(item => window.__lizardReviveWire(item, depth + 1));
                            if (value && typeof value === 'object')
                                return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, window.__lizardReviveWire(item, depth + 1)]));
                            return value;
                        };
                        const nativePost = window.postMessage.bind(window);
                        window.postMessage = function(data, targetOrigin, transfer) {
                            if (data && Object.prototype.hasOwnProperty.call(data, 'pluginMessage')) {
                                window.ipc.postMessage(JSON.stringify({ kind: 'pluginMessage', payload: encodeWire(data.pluginMessage) }));
                                return;
                            }
                            return nativePost(data, targetOrigin, transfer);
                        };
                    })();"#,
                )
                .with_ipc_handler(move |request| {
                    if request.body().len() > lizard_figma_plugin::worker_ui::MAX_UI_MESSAGE_BYTES {
                        return;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(request.body()) else { return };
                    if value["kind"] == "ready" {
                        ipc_inbox.borrow_mut().push(Event::Ready);
                        ipc_ctx.request_repaint();
                        return;
                    }
                    if value["kind"] == "diagnostic" {
                        if std::env::var_os("LIZARD_PLUGIN_DEBUG").is_some() {
                            eprintln!("plugin UI: {}", value["message"].as_str().unwrap_or("unknown error"));
                        }
                        return;
                    }
                    if value["kind"] == "pluginMessage"
                        && let Some(payload) = value.get("payload")
                    {
                        if std::env::var_os("LIZARD_PLUGIN_DEBUG").is_some() {
                            eprintln!("plugin UI receive: {}", payload["type"].as_str().unwrap_or("unknown"));
                        }
                        let mut inbox = ipc_inbox.borrow_mut();
                        if inbox.len() < 128 {
                            inbox.push(Event::Message(payload.clone()));
                            ipc_ctx.request_repaint();
                        }
                    }
                })
                .build_as_child(window.as_ref())
                .map_err(|e| format!("cannot open plugin HTML: {e}"))?;
            *slot = Some(Self {
                id: snapshot.id,
                html: snapshot.html,
                allowed_domains: snapshot.allowed_domains,
                view,
                ready: false,
                pending: snapshot.outgoing,
            });
            return Ok(());
        }
        let Some(open) = slot.as_mut() else { return Ok(()) };
        open.view.set_bounds(bounds).map_err(|e| format!("cannot position plugin HTML: {e}"))?;
        open.view.set_visible(true).map_err(|e| format!("cannot show plugin HTML: {e}"))?;
        open.pending.extend(snapshot.outgoing);
        if open.ready {
            for message in open.pending.drain(..) {
                if std::env::var_os("LIZARD_PLUGIN_DEBUG").is_some() {
                    eprintln!("plugin UI send: {}", message["type"].as_str().unwrap_or("unknown"));
                }
                let encoded = serde_json::to_string(&message).map_err(|e| format!("cannot encode plugin message: {e}"))?;
                open.view
                    .evaluate_script(&format!(
                        "window.dispatchEvent(new MessageEvent('message', {{ data: {{ pluginMessage: window.__lizardReviveWire({encoded}) }} }}));"
                    ))
                    .map_err(|e| format!("cannot deliver plugin message: {e}"))?;
            }
        }
        Ok(())
    }
}

/// Keep the original doctype and head in place so a plugin UI retains its layout mode.
fn html_with_policy(html: &str, policy: &str) -> String {
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">");
    let lower = html.to_ascii_lowercase();
    if let Some(head) = lower.find("<head")
        && let Some(end) = lower[head..].find('>')
    {
        let at = head + end + 1;
        return format!("{}{}{}", &html[..at], meta, &html[at..]);
    }
    if let Some(doctype) = lower.find("<!doctype")
        && let Some(end) = lower[doctype..].find('>')
    {
        let at = doctype + end + 1;
        return format!("{}<head>{meta}</head>{}", &html[..at], &html[at..]);
    }
    format!("<head>{meta}</head>{html}")
}

/// Grant only the network origins declared by the plugin manifest. Navigation stays blocked.
fn stylesheet_origins(domains: &[String]) -> String {
    let mut origins = std::collections::BTreeSet::new();
    for domain in domains {
        if domain == "*" {
            origins.insert("https:".to_owned());
            origins.insert("wss:".to_owned());
            continue;
        }
        let Some((scheme, rest)) =
            domain.strip_prefix("https://").map(|rest| ("https://", rest)).or_else(|| domain.strip_prefix("wss://").map(|rest| ("wss://", rest)))
        else {
            continue;
        };
        if !rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~' | b'/' | b':' | b'%' | b'?' | b'=' | b'&' | b'*'))
        {
            continue;
        }
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let suffix = host.strip_prefix("*.");
        let valid_host = suffix.unwrap_or(host);
        if !valid_host.is_empty()
            && valid_host.contains('.')
            && valid_host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        {
            origins.insert(format!("{scheme}{host}"));
        }
    }
    origins.into_iter().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{html_with_policy, stylesheet_origins};

    #[test]
    fn policy_preserves_plugin_doctype_and_head() {
        let html = "<!DOCTYPE html><html><head><title>Plugin</title></head><body>UI</body></html>";
        let secured = html_with_policy(html, "default-src 'none'");
        assert!(secured.starts_with("<!DOCTYPE html><html><head><meta http-equiv="));
        assert!(secured.contains("<title>Plugin</title>"));
        assert!(secured.ends_with("<body>UI</body></html>"));
    }

    #[test]
    fn only_manifest_https_hosts_enter_stylesheet_policy() {
        let sources = stylesheet_origins(&[
            "https://static.figma.com/api/figma-extension-api-0.0.1.css".into(),
            "https://fonts.gstatic.com".into(),
            "https://static.figma.com/another.css".into(),
            "https://bad.example/\"; script-src *".into(),
            "http://insecure.example".into(),
        ]);
        assert_eq!(sources, "https://fonts.gstatic.com https://static.figma.com");
    }

    #[test]
    fn wildcard_and_websocket_origins_keep_the_manifest_scope() {
        let sources = stylesheet_origins(&[
            "https://*.icons8.com/path".into(),
            "wss://socket.example.com/path".into(),
            "https://bad.example/\"; connect-src *".into(),
        ]);
        assert_eq!(sources, "https://*.icons8.com wss://socket.example.com");
    }
}
