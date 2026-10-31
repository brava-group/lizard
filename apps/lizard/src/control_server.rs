//! Localhost JSON-lines control server: each line `{"id", "method", "params"}` is queued for the UI
//! thread; the reply line `{"id", "ok", "result" | "error"}` is written when the frame handled it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lizard_ui_egui::ControlRequest;
use serde_json::{Value, json};

type Queue = Arc<Mutex<Vec<ControlRequest>>>;
type CtxSlot = Arc<Mutex<Option<egui::Context>>>;

const MAX_LINE: usize = 32 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 8;
const MAX_QUEUED_REQUESTS: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(60);

/// Deferred requests for a hidden window must not make the UI's queue grow without a bound.
pub fn drain_queue(queue: &Queue, target: &mut Vec<ControlRequest>) {
    target.retain(|request| !request.is_cancelled());
    let mut queue = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    queue.retain(|request| !request.is_cancelled());
    let count = queue.len().min(MAX_QUEUED_REQUESTS.saturating_sub(target.len()));
    target.extend(queue.drain(..count));
}

pub fn start(port: u16, queue: Queue, ctx: CtxSlot) -> Option<u16> {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server: can't bind 127.0.0.1:{port}: {e} (another app may own it; pick another port)");
            eprintln!("Lizard control server failed to bind port {port}: {e}");
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    eprintln!("Lizard control server listening on 127.0.0.1:{port}");
    let spawned = std::thread::Builder::new().name("lizard-control".into()).spawn(move || {
        let mut workers = Vec::new();
        for stream in listener.incoming().flatten() {
            spawn_connection(stream, queue.clone(), ctx.clone(), &mut workers, TIMEOUT);
        }
    });
    if let Err(e) = spawned {
        log::error!("control server thread: {e}");
        return None;
    }
    Some(port)
}

/// A private MCP bridge tied to the document that started the agent. Dropping its flag revokes
/// queued and future calls, including calls from an old agent process still shutting down.
pub fn start_agent(document: u64, queue: Queue, ctx: CtxSlot) -> Option<(u16, Arc<AtomicBool>)> {
    start_agent_scoped(document, 0, queue, ctx)
}

pub fn start_agent_scoped(document: u64, agent_index: usize, queue: Queue, ctx: CtxSlot) -> Option<(u16, Arc<AtomicBool>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    listener.set_nonblocking(true).ok()?;
    let enabled = Arc::new(AtomicBool::new(true));
    let flag = enabled.clone();
    std::thread::Builder::new()
        .name("lizard-agent-control".into())
        .spawn(move || {
            let mut workers = Vec::new();
            while flag.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        workers.retain(|worker: &std::thread::JoinHandle<()>| !worker.is_finished());
                        if workers.len() >= MAX_CONNECTIONS {
                            continue;
                        }
                        let queue = queue.clone();
                        let ctx = ctx.clone();
                        let guard = flag.clone();
                        if let Ok(worker) = std::thread::Builder::new().name("lizard-agent-control-conn".into()).spawn(move || {
                            let _ = serve_guarded(stream, queue, ctx, TIMEOUT, Some((document, agent_index, guard)));
                        }) {
                            workers.push(worker);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
        })
        .ok()?;
    Some((port, enabled))
}

fn spawn_connection(stream: TcpStream, queue: Queue, ctx: CtxSlot, workers: &mut Vec<std::thread::JoinHandle<()>>, timeout: Duration) {
    workers.retain(|worker| !worker.is_finished());
    if workers.len() >= MAX_CONNECTIONS {
        return;
    }
    match std::thread::Builder::new().name("lizard-control-conn".into()).spawn(move || {
        let _ = serve(stream, queue, ctx, timeout);
    }) {
        Ok(worker) => workers.push(worker),
        Err(e) => log::error!("control connection thread: {e}"),
    }
}

fn serve(out: TcpStream, queue: Queue, ctx: CtxSlot, timeout: Duration) -> std::io::Result<()> {
    serve_guarded(out, queue, ctx, timeout, None)
}

fn serve_guarded(
    mut out: TcpStream,
    queue: Queue,
    ctx: CtxSlot,
    timeout: Duration,
    guard: Option<(u64, usize, Arc<AtomicBool>)>,
) -> std::io::Result<()> {
    out.set_read_timeout(Some(timeout))?;
    out.set_write_timeout(Some(timeout))?;
    let mut reader = BufReader::new(out.try_clone()?);
    let mut line = Vec::new();
    loop {
        line.clear();
        if (&mut reader).take((MAX_LINE + 1) as u64).read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.len() > MAX_LINE {
            writeln!(out, "{}", json!({"id": null, "ok": false, "error": "message too large"}))?;
            break;
        }
        let Ok(line) = std::str::from_utf8(&line) else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(v) => {
                let id = v.get("id").cloned().unwrap_or(Value::Null);
                let method = v.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let mut params = v.get("params").cloned().unwrap_or(json!({}));
                if let Some((document, agent_index, enabled)) = &guard {
                    if !enabled.load(Ordering::Acquire) {
                        writeln!(out, "{}", json!({"id": id, "ok": false, "error": "A sessão do agente foi encerrada"}))?;
                        continue;
                    }
                    if let Some(object) = params.as_object_mut() {
                        object.insert("__agent_document".into(), json!(document));
                        object.insert("__agent_index".into(), json!(agent_index));
                    }
                }
                let (tx, rx) = std::sync::mpsc::channel();
                let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let mut pending = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                pending.retain(|request| !request.is_cancelled());
                if pending.len() >= MAX_QUEUED_REQUESTS {
                    drop(pending);
                    writeln!(out, "{}", json!({"id": id, "ok": false, "error": "control request queue is full"}))?;
                    continue;
                }
                pending.push(ControlRequest {
                    id: id.clone(),
                    method,
                    params,
                    reply: tx,
                    cancelled: cancelled.clone(),
                    agent_guard: guard.as_ref().map(|(_, _, flag)| flag.clone()),
                });
                drop(pending);
                if let Some(c) = ctx.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
                    c.request_repaint();
                }
                match rx.recv_timeout(timeout) {
                    Ok(r) => match r.result {
                        Ok(res) => json!({"id": r.id, "ok": true, "result": res}),
                        Err(e) => json!({"id": r.id, "ok": false, "error": e}),
                    },
                    Err(_) => {
                        cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                        json!({"id": id, "ok": false, "error": "timed out waiting for the UI: screenshots and input need the Lizard window on screen (is it minimised or covered?)"})
                    }
                }
            }
            Err(e) => {
                // A line that isn't JSON ends the connection: a web page can make the browser send
                // an HTTP request here, and its header lines must not let the JSON lines of its body
                // through as commands.
                writeln!(out, "{}", json!({"id": null, "ok": false, "error": format!("bad JSON: {e}")}))?;
                break;
            }
        };
        writeln!(out, "{reply}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "control_server_tests.rs"]
mod tests;
