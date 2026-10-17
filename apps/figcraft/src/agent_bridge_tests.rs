//! The in-app agent's MCP endpoint drives the current document and rejects a stale document.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use figcraft_engine::Session;
use figcraft_ui_egui::control::ControlReply;
use figcraft_ui_egui::{FigApp, NoServices};
use serde_json::json;

use crate::control_server;

#[test]
fn agent_mcp_creates_a_layer_in_the_active_document_and_revokes_its_bridge() {
    let mut app = FigApp::new(Session::new(), Box::new(NoServices));
    let uid = app.session.ds().unwrap().uid;
    let queue = Arc::new(Mutex::new(Vec::new()));
    let ctx = egui::Context::default();
    let slot = Arc::new(Mutex::new(Some(ctx.clone())));
    let (port, enabled) = control_server::start_agent(uid, queue.clone(), slot).unwrap();
    let (done, finished) = mpsc::channel();
    let (revoked, check_revocation) = mpsc::channel();
    std::thread::spawn(move || {
        let mut attached = figcraft_mcp::Attached::port(port).unwrap();
        let created = figcraft_mcp::call_tool(&mut attached, "create_node", &json!({"type":"frame","name":"Agent test","width":240,"height":120}));
        done.send(created).unwrap();
        if check_revocation.recv_timeout(Duration::from_secs(2)).is_ok() {
            let rejected = figcraft_mcp::call_tool(&mut attached, "create_node", &json!({"type":"frame","name":"Stale agent edit"}));
            done.send(rejected).unwrap();
        }
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let created = loop {
        if let Ok(result) = finished.try_recv() {
            break result;
        }
        assert!(Instant::now() < deadline, "MCP attached call did not finish");
        let requests = std::mem::take(&mut *queue.lock().unwrap());
        for request in requests {
            let result = figcraft_ui_egui::control::handle(&mut app, &ctx, &request.method, &request.params);
            request.reply.send(ControlReply { id: request.id, result }).unwrap();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(!created.is_error, "{created:?}");
    assert!(app.session.ds().unwrap().history.undo.last().unwrap().agent);
    enabled.store(false, Ordering::Release);
    revoked.send(()).unwrap();
    let rejected = finished.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(rejected.is_error, "A revogação aceitou uma edição: {rejected:?}");
    assert_eq!(app.session.ds().unwrap().history.undo.len(), 1);
}
