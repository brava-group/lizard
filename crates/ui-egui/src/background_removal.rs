//! Native bridge to withoutBG's local open-weights model.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use base64::Engine as _;
use lizard_color::PaintKind;
use lizard_doc::NodeId;
use serde_json::json;

use crate::LizardApp;
use crate::i18n::tr;

pub(crate) struct Pending {
    doc_uid: u64,
    id: NodeId,
    index: usize,
    source_hash: String,
    rx: Receiver<Result<Vec<u8>, String>>,
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
        let result = run_model(&bytes);
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    if let Err(error) = spawn {
        app.show_error(format!("withoutBG: {error}"));
        return;
    }
    app.pending_background_removal = Some(Pending { doc_uid, id, index, source_hash, rx });
    app.toast(tr("Removing image background locally…"));
}

pub(crate) fn poll(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_background_removal.take() else { return };
    match pending.rx.try_recv() {
        Err(TryRecvError::Empty) => {
            app.pending_background_removal = Some(pending);
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        Err(TryRecvError::Disconnected) => app.show_error(tr("Background removal stopped unexpectedly").to_string()),
        Ok(Err(error)) => app.show_error(format!("withoutBG: {error}")),
        Ok(Ok(png)) => {
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
        }
    }
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

fn python() -> Result<PathBuf, String> {
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

fn run_model(bytes: &[u8]) -> Result<Vec<u8>, String> {
    const SCRIPT: &str = "import io, sys\nfrom PIL import Image\nfrom withoutbg import WithoutBG\nsource = Image.open(io.BytesIO(sys.stdin.buffer.read()))\nresult = WithoutBG.open_weights().remove_background(source)\nresult.convert('RGBA').save(sys.stdout.buffer, format='PNG')\n";
    let interpreter = python()?;
    let mut child = Command::new(interpreter)
        .args(["-c", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start model: {e}"))?;
    if let Some(mut input) = child.stdin.take() {
        input.write_all(bytes).map_err(|e| format!("cannot send image: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| format!("model failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.chars().take(1200).collect());
    }
    if !output.stdout.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("model did not return a PNG image".into());
    }
    Ok(output.stdout)
}
