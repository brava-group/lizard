//! macOS child WebView for a local Figma plugin's self-contained HTML.
//! The WebView has no document access: only JSON pluginMessage values reach engine commands.

use std::cell::RefCell;
use std::rc::Rc;

use egui::Rect;
use serde_json::Value;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder};

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
            let ready_inbox = inbox.clone();
            let ready_ctx = ctx.clone();
            let style_origins = stylesheet_origins(&snapshot.allowed_domains);
            let html = format!(
                "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: blob:; font-src data: {style_origins}; style-src 'unsafe-inline' {style_origins}; script-src 'unsafe-inline'; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'\">{html}",
                html = snapshot.html
            );
            let view = WebViewBuilder::new()
                .with_bounds(bounds)
                .with_html(html)
                .with_incognito(true)
                .with_clipboard(false)
                .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                .with_navigation_handler(|url| url == "about:blank")
                .with_initialization_script(
                    r#"(() => {
                        const nativePost = window.postMessage.bind(window);
                        window.postMessage = function(data, targetOrigin, transfer) {
                            if (data && Object.prototype.hasOwnProperty.call(data, 'pluginMessage')) {
                                window.ipc.postMessage(JSON.stringify({ kind: 'pluginMessage', payload: data.pluginMessage }));
                                return;
                            }
                            return nativePost(data, targetOrigin, transfer);
                        };
                    })();"#,
                )
                .with_ipc_handler(move |request| {
                    if request.body().len() > figcraft_figma_plugin::worker_ui::MAX_UI_MESSAGE_BYTES {
                        return;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(request.body()) else { return };
                    if value["kind"] == "pluginMessage"
                        && let Some(payload) = value.get("payload")
                    {
                        let mut inbox = ipc_inbox.borrow_mut();
                        if inbox.len() < 128 {
                            inbox.push(Event::Message(payload.clone()));
                            ipc_ctx.request_repaint();
                        }
                    }
                })
                .with_on_page_load_handler(move |event, _| {
                    if matches!(event, PageLoadEvent::Finished) {
                        ready_inbox.borrow_mut().push(Event::Ready);
                        ready_ctx.request_repaint();
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
                let encoded = serde_json::to_string(&message).map_err(|e| format!("cannot encode plugin message: {e}"))?;
                open.view
                    .evaluate_script(&format!("window.dispatchEvent(new MessageEvent('message', {{ data: {{ pluginMessage: {encoded} }} }}));"))
                    .map_err(|e| format!("cannot deliver plugin message: {e}"))?;
            }
        }
        Ok(())
    }
}

/// A plugin's manifest may grant HTTPS resources. The HTML preview uses that grant only for
/// stylesheets and fonts; scripts, navigation and arbitrary network requests remain blocked.
fn stylesheet_origins(domains: &[String]) -> String {
    let mut origins = std::collections::BTreeSet::new();
    for domain in domains {
        let Some(rest) = domain.strip_prefix("https://") else { continue };
        if !rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~' | b'/' | b':' | b'%' | b'?' | b'=' | b'&'))
        {
            continue;
        }
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if !host.is_empty() && host.contains('.') && host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')) {
            origins.insert(format!("https://{host}"));
        }
    }
    origins.into_iter().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::stylesheet_origins;

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
}
