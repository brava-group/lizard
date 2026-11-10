//! Native bridge to withoutBG's local open-weights model.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use base64::Engine as _;
use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind, pos2, vec2};
use lizard_color::PaintKind;
use lizard_doc::NodeId;
use serde_json::json;
use web_time::Instant;

use crate::LizardApp;
use crate::i18n::{tr, tr_noop};
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Preparing,
    Installing,
    LoadingModel,
    Removing,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Preparing => tr_noop("Preparing local image processor…"),
            Self::Installing => tr_noop("Installing local image processor…"),
            Self::LoadingModel => tr_noop("Preparing local model (first use may download)…"),
            Self::Removing => tr_noop("Removing image background locally…"),
        }
    }
}

enum Update {
    Phase(Phase),
    Done(Result<Vec<u8>, String>),
}

pub(crate) struct Pending {
    doc_uid: u64,
    id: NodeId,
    index: usize,
    source_hash: String,
    phase: Phase,
    started: Instant,
    rx: Receiver<Update>,
}

pub(crate) fn start(app: &mut LizardApp, ctx: &egui::Context, id: NodeId, index: usize) {
    if app.pending_background_removal.is_some() {
        app.toast(tr("Background removal is already running"));
        return;
    }
    let Some((source_hash, bytes)) = app.session.doc().ok().and_then(|doc| {
        let node = doc.node(id)?;
        let PaintKind::Image(image) = &node.fills.get(index)?.kind else { return None };
        let data = doc.images.get(&image.image)?;
        Some((image.image.clone(), data.bytes.clone()))
    }) else {
        app.show_error(tr("Selected image data is unavailable").to_string());
        return;
    };
    let doc_uid = app.session.ds().map(|ds| ds.uid).unwrap_or(0);
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    let spawn = std::thread::Builder::new().name("withoutbg".into()).spawn(move || {
        let result = run_model(&bytes, &tx);
        let _ = tx.send(Update::Done(result));
        ctx.request_repaint();
    });
    if let Err(error) = spawn {
        app.show_error(format!("withoutBG: {error}"));
        return;
    }
    app.pending_background_removal = Some(Pending { doc_uid, id, index, source_hash, phase: Phase::Preparing, started: Instant::now(), rx });
    app.toast(tr(Phase::Preparing.label()));
}

pub(crate) fn poll(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(mut pending) = app.pending_background_removal.take() else { return };
    loop {
        match pending.rx.try_recv() {
            Ok(Update::Phase(phase)) => {
                if phase != pending.phase {
                    pending.phase = phase;
                    app.toast(tr(phase.label()));
                }
            }
            Ok(Update::Done(Err(error))) => {
                log::warn!("background removal failed: {error}");
                let reason = error.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("unknown error");
                app.show_error(format!("{}: {}", tr("Background removal failed"), reason.chars().take(180).collect::<String>()));
                return;
            }
            Ok(Update::Done(Ok(png))) => {
                if app.session.ds().map(|ds| ds.uid).unwrap_or(0) != pending.doc_uid {
                    app.toast(tr("Background removal canceled because the document changed"));
                    return;
                }
                let data = base64::engine::general_purpose::STANDARD.encode(png);
                if app
                    .run("image.replaceBytes", json!({"id": pending.id, "index": pending.index, "data": data, "expectedImage": pending.source_hash}))
                    .is_some()
                {
                    app.toast(tr("Background removed"));
                }
                return;
            }
            Err(TryRecvError::Empty) => {
                app.pending_background_removal = Some(pending);
                ctx.request_repaint_after(Duration::from_millis(250));
                return;
            }
            Err(TryRecvError::Disconnected) => {
                app.show_error(tr("Background removal stopped unexpectedly").to_string());
                return;
            }
        }
    }
}

/// Match the agent's active frame treatment while local image processing is running.
pub(crate) fn draw(app: &LizardApp, painter: &Painter, ctx: &egui::Context) {
    let Some(pending) = app.pending_background_removal.as_ref() else { return };
    let Some(bounds) = app.session.ds().ok().filter(|ds| ds.uid == pending.doc_uid).and_then(|ds| ds.doc.world_bounds(pending.id)) else {
        return;
    };
    let rect = Rect::from_two_pos(app.to_screen(bounds.x0, bounds.y0), app.to_screen(bounds.x1, bounds.y1));
    if !rect.is_positive() || !rect.intersects(app.canvas.rect) {
        return;
    }
    let elapsed = pending.started.elapsed().as_secs_f32();
    let brand = Tokens::get(ctx).accent;
    let clipped = painter.with_clip_rect(app.canvas.rect);
    crate::agent_presence::draw_frame_border(&clipped, rect, brand, elapsed, 1.0, app.reduce_motion());

    let label = format!("{} · {}s", tr(pending.phase.label()), elapsed as u64);
    let galley = clipped.layout_no_wrap(label, crate::theme::semibold(11.0), Color32::WHITE);
    let badge_width = galley.size().x + 20.0;
    let x = (rect.left() + 8.0).min(app.canvas.rect.right() - badge_width - 8.0).max(app.canvas.rect.left() + 8.0);
    let y = (rect.top() + 8.0).max(app.canvas.rect.top() + 8.0);
    let badge = Rect::from_min_size(pos2(x, y), vec2(badge_width, 26.0));
    clipped.rect(badge, CornerRadius::same(7), Color32::from_black_alpha(190), Stroke::new(1.0, brand), StrokeKind::Inside);
    clipped.galley(pos2(badge.left() + 10.0, badge.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    ctx.request_repaint_after(Duration::from_millis(if app.reduce_motion() { 250 } else { 33 }));
}

fn python_in(venv: &std::path::Path) -> PathBuf {
    if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") }
}

fn ready(python: &std::ffi::OsStr) -> bool {
    Command::new(python)
        .args(["-c", "from withoutbg import WithoutBG; assert hasattr(WithoutBG, 'open_weights')"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn python(tx: &mpsc::Sender<Update>) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("LIZARD_WITHOUTBG_PYTHON") {
        return if ready(&path) { Ok(PathBuf::from(path)) } else { Err("LIZARD_WITHOUTBG_PYTHON cannot import withoutbg".into()) };
    }
    if ready(std::ffi::OsStr::new("python3")) {
        return Ok(PathBuf::from("python3"));
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .ok_or("No cache directory available for withoutBG")?;
    let venv = base.join("lizard").join("withoutbg-venv");
    let interpreter = python_in(&venv);
    if ready(interpreter.as_os_str()) {
        return Ok(interpreter);
    }
    let _ = tx.send(Update::Phase(Phase::Installing));
    std::fs::create_dir_all(&venv).map_err(|e| format!("cannot create Python environment: {e}"))?;
    let created = Command::new("python3").args(["-m", "venv"]).arg(&venv).output().map_err(|e| format!("Python 3 is required: {e}"))?;
    if !created.status.success() {
        return Err(format!("cannot create Python environment: {}", String::from_utf8_lossy(&created.stderr)));
    }
    let installed = Command::new(&interpreter)
        .args(["-m", "pip", "install", "withoutbg==1.3.0"])
        .output()
        .map_err(|e| format!("cannot install withoutbg: {e}"))?;
    if !installed.status.success() {
        return Err(format!("cannot install withoutbg: {}", String::from_utf8_lossy(&installed.stderr)));
    }
    Ok(interpreter)
}

fn run_model(bytes: &[u8], tx: &mpsc::Sender<Update>) -> Result<Vec<u8>, String> {
    const SCRIPT: &str = "import io, sys\nfrom PIL import Image\nfrom withoutbg import WithoutBG\nsource = Image.open(io.BytesIO(sys.stdin.buffer.read()))\nmodel = WithoutBG.open_weights()\nmodel.preload()\nprint('__LIZARD_BACKGROUND_PROCESSING__', file=sys.stderr, flush=True)\nresult = model.remove_background(source)\nresult.convert('RGBA').save(sys.stdout.buffer, format='PNG')\n";
    let interpreter = python(tx)?;
    let _ = tx.send(Update::Phase(Phase::LoadingModel));
    let mut child = Command::new(interpreter)
        .args(["-c", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start model: {e}"))?;
    let stderr = child.stderr.take().ok_or("cannot read model progress")?;
    let progress = tx.clone();
    let log_reader = std::thread::spawn(move || {
        let mut messages = VecDeque::new();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if line == "__LIZARD_BACKGROUND_PROCESSING__" {
                let _ = progress.send(Update::Phase(Phase::Removing));
            } else {
                messages.push_back(line.chars().take(512).collect::<String>());
                if messages.len() > 12 {
                    messages.pop_front();
                }
            }
        }
        messages.into_iter().collect::<Vec<_>>().join("\n")
    });
    if let Some(mut input) = child.stdin.take()
        && let Err(error) = input.write_all(bytes)
    {
        let _ = child.wait();
        let _ = log_reader.join();
        return Err(format!("cannot send image: {error}"));
    }
    let output = child.wait_with_output();
    let messages = log_reader.join().map_err(|_| "cannot read model progress")?;
    let output = output.map_err(|e| format!("model failed: {e}"))?;
    if !output.status.success() {
        return Err(if messages.is_empty() { format!("model exited with {}", output.status) } else { messages });
    }
    if !output.stdout.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("model did not return a PNG image".into());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lizard_engine::Session;

    struct NoServices;
    impl crate::Services for NoServices {}

    #[test]
    fn model_failure_clears_progress_and_shows_a_short_error_toast() {
        let mut app = LizardApp::new(Session::new(), Box::new(NoServices));
        let (tx, rx) = mpsc::channel();
        app.pending_background_removal = Some(Pending {
            doc_uid: app.session.ds().map_or(0, |ds| ds.uid),
            id: NodeId(0),
            index: 0,
            source_hash: String::new(),
            phase: Phase::Preparing,
            started: Instant::now(),
            rx,
        });
        let ctx = egui::Context::default();
        let _ = tx.send(Update::Phase(Phase::LoadingModel));
        poll(&mut app, &ctx);
        assert!(app.pending_background_removal.as_ref().is_some_and(|pending| pending.phase == Phase::LoadingModel));

        let _ = tx.send(Update::Done(Err("Traceback details\nNetwork unavailable".into())));
        poll(&mut app, &ctx);
        assert!(app.pending_background_removal.is_none());
        assert!(app.ui.last_error.as_deref().is_some_and(|message| message.contains("Network unavailable")));
        assert!(app.ui.toast.as_ref().is_some_and(|(message, _)| message.contains("Network unavailable")));
    }
}
