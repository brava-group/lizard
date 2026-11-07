//! Desktop services: native file dialogs, clipboard, URLs, persisted preferences.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lizard_engine::{Prefs, Session};
use lizard_ui_egui::agent_panel::{AgentEvent, AgentKind, AgentPrompt};
use lizard_ui_egui::i18n::{tr, trf};
use lizard_ui_egui::{ControlRequest, DialogFuture, LizardApp, Services, UpdateStatus};
use serde_json::{Value, json};

type ControlQueue = Arc<Mutex<Vec<ControlRequest>>>;
type ContextSlot = Arc<Mutex<Option<egui::Context>>>;

pub struct Desktop {
    #[cfg(target_os = "macos")]
    updater: crate::updater::Updater,
    clipboard: Option<arboard::Clipboard>,
    /// Where the preferences live ([`config_dir`]).
    dir: Option<PathBuf>,
    pub prefs: Option<Prefs>,
    /// The system's reduce-motion setting (read in the background at startup).
    reduce_motion: Arc<AtomicBool>,
    /// The system's interface language (read at startup).
    language: Option<String>,
    /// Recovery copies of unsaved files (written at once after a panic).
    recovery: crate::recovery::Shared,
    agents: Vec<crate::agent::Agent>,
    reviewer: Option<crate::agent::Agent>,
    reviewer_busy: bool,
    reviewer_agent_index: usize,
    reviewer_text: String,
    reviewer_pending: Vec<(u64, usize, String, Vec<u64>)>,
    reviewer_template: Option<AgentPrompt>,
    pending_parallel: Option<(AgentKind, AgentPrompt, u64)>,
    agent_control: Option<(ControlQueue, ContextSlot)>,
    agent_bridges: Vec<(u16, Arc<AtomicBool>)>,
    agent_login_events: Option<std::sync::mpsc::Receiver<AgentEvent>>,
    agent_login_kind: Option<AgentKind>,
    agent_login_attempt: Option<crate::agent::LoginAttempt>,
    agent_signed_in: [bool; 4],
    agent_auth_checked: [Option<bool>; 4],
    agent_auth_probe_started: [bool; 4],
    agent_auth_probe_tx: std::sync::mpsc::Sender<(AgentKind, bool)>,
    agent_auth_probe_rx: std::sync::mpsc::Receiver<(AgentKind, bool)>,
    agent_model_events: Option<std::sync::mpsc::Receiver<AgentEvent>>,
}

pub fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|b| b.join("Lizard"))
}

impl Desktop {
    pub fn load(recovery: crate::recovery::Shared) -> Self {
        let dir = config_dir();
        if let Some(current) = &dir {
            let current_state = current.join(STATE);
            if !current_state.exists()
                && let Some(legacy) = current.parent().map(|base| base.join("Lizard").join(STATE))
                && let Ok(bytes) = std::fs::read(legacy)
                && std::fs::create_dir_all(current).is_ok()
            {
                let _ = std::fs::write(current_state, bytes);
            }
        }
        Self::load_from(dir, recovery)
    }

    /// With the preferences kept in `dir`.
    pub fn load_from(dir: Option<PathBuf>, recovery: crate::recovery::Shared) -> Self {
        let state: Value =
            dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(STATE)).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
        let prefs = state.get("prefs").and_then(|p| serde_json::from_value::<Prefs>(p.clone()).ok()).map(|mut p| {
            // Older builds saved the inherited public server as a preference. Do not reconnect
            // to it implicitly after Lizard moves to user-configured hosting.
            if p.publish_server == "wss://collab.craftapps.dev/" {
                p.publish_server.clear();
            }
            // Recent files lived next to the preferences before they moved into them.
            if p.recent_files.is_empty()
                && let Some(old) = state.get("recents").and_then(Value::as_array)
            {
                p.recent_files = old.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
            }
            p.recent_files.truncate(lizard_engine::cmd::file::MAX_RECENT);
            p
        });
        let reduce_motion = Arc::new(AtomicBool::new(false));
        let (agent_auth_probe_tx, agent_auth_probe_rx) = std::sync::mpsc::channel();
        let flag = reduce_motion.clone();
        std::thread::spawn(move || flag.store(system_reduce_motion(), Ordering::Relaxed));
        Self {
            #[cfg(target_os = "macos")]
            updater: crate::updater::Updater::new(),
            clipboard: arboard::Clipboard::new().ok(),
            dir,
            prefs,
            reduce_motion,
            language: system_language(),
            recovery,
            agents: Vec::new(),
            reviewer: None,
            reviewer_busy: false,
            reviewer_agent_index: 0,
            reviewer_text: String::new(),
            reviewer_pending: Vec::new(),
            reviewer_template: None,
            pending_parallel: None,
            agent_control: None,
            agent_bridges: Vec::new(),
            agent_login_events: None,
            agent_login_kind: None,
            agent_login_attempt: None,
            agent_signed_in: [false; 4],
            agent_auth_checked: [None; 4],
            agent_auth_probe_started: [false; 4],
            agent_auth_probe_tx,
            agent_auth_probe_rx,
            agent_model_events: None,
        }
    }

    pub fn configure_agent(&mut self, queue: ControlQueue, context: ContextSlot) {
        self.agent_control = Some((queue, context));
    }

    fn agent_port(&mut self, document: u64, index: usize) -> Result<u16, String> {
        let Some((queue, context)) = &self.agent_control else { return Err(tr("The control channel is unavailable").into()) };
        let bridge = if index == 0 {
            crate::control_server::start_agent(document, queue.clone(), context.clone())
        } else {
            crate::control_server::start_agent_scoped(document, index, queue.clone(), context.clone())
        };
        let (port, enabled) = bridge.ok_or(tr("Could not open Lizard's control channel"))?;
        self.agent_bridges.push((port, enabled));
        Ok(port)
    }
}

/// The system's preferred interface languages, most preferred first, comma-separated
/// ("nl-NL,de-DE,en-US"): Windows' language list, macOS's AppleLanguages, `LANGUAGE` and the
/// locale variables elsewhere. The UI shows the first language in the list that has a catalog.
/// Windows keeps only its first preferred language (and English when it has no catalog): that
/// list is also the input-language list, so a language added for a keyboard (Deutsch on a Polish
/// system) must not decide the interface language.
fn system_language() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows::System::UserProfile::GlobalizationPreferences::Languages()
            .and_then(|l| l.GetAt(0))
            .map(|s| s.to_string())
            .inspect_err(|e| log::info!("language: can't read the system languages: {e}"))
            .ok()
    }
    // `defaults` prints a list: ( "es-ES", en ).
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output().ok()?;
        let list: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .split(['(', ',', ')'])
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Some(list.join(","))
    }
    // gettext's `LANGUAGE` priority list ("fr:de"), then the locale ("fr_FR.UTF-8").
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty() && v != "C" && v != "POSIX");
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(var);
        let list: Vec<String> = var("LANGUAGE").map(|l| l.replace(':', ",")).into_iter().chain(locale).collect();
        (!list.is_empty()).then(|| list.join(","))
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// Whether the system's accessibility settings ask for reduced motion: Windows' animation
/// effects (`UISettings.AnimationsEnabled`), macOS's Reduce motion, GNOME's animations.
fn system_reduce_motion() -> bool {
    #[cfg(target_os = "windows")]
    {
        match windows::UI::ViewManagement::UISettings::new().and_then(|s| s.AnimationsEnabled()) {
            Ok(on) => !on,
            Err(e) => {
                log::info!("reduce motion: can't read the animation setting: {e}");
                false
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        command_says("defaults", &["read", "com.apple.universalaccess", "reduceMotion"], "1")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        command_says("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"], "false")
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

/// Whether `program args` runs and prints `want` (trimmed).
#[cfg(unix)]
fn command_says(program: &str, args: &[&str], want: &str) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == want)
}

/// Show `path` in the file manager: selected in Finder and Explorer, its folder elsewhere.
fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        let mut c = std::process::Command::new("explorer");
        c.arg(arg);
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    // The file manager keeps running; reap it off this thread so it leaves no zombie.
    let mut child = command.spawn()?;
    let _ = std::thread::Builder::new().name("lizard-reveal".into()).spawn(move || child.wait());
    Ok(())
}

/// The preferences' file in the config folder.
const STATE: &str = "state.json";

/// Everything quitting keeps, all done when it returns (macOS then ends the process): the
/// preferences in `dir`, this run's recovery copies ([`Recovery::quit`](crate::recovery::Recovery::quit))
/// and the live sessions' ends (one this app hosts tells everyone and stops; a joined one leaves).
pub fn shutdown(app: &mut LizardApp, dir: Option<&Path>, recovery: Option<crate::recovery::Recovery>) {
    app.services.update_cleanup();
    if let Some(dir) = dir {
        write_prefs(dir, &app.session.prefs);
    }
    // Before the sessions end: a recovered online file's copy waits for its session's outcome.
    if let Some(r) = recovery {
        r.quit(&app.session, app.ui.quit);
    }
    drop(std::mem::take(&mut app.session.live));
}

/// Write `prefs` to `dir` atomically (a temporary file renamed over the old one: a crash leaves
/// either version whole). They hold online owner tokens, so on Unix only the owner can read them.
pub(crate) fn write_prefs(dir: &Path, prefs: &Prefs) {
    let tmp = dir.join("state.json.tmp");
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| write_private(&tmp, json!({"prefs": prefs}).to_string().as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, dir.join(STATE)));
    if let Err(e) = written {
        log::warn!("preferences: couldn't save them in {}: {e}", dir.display());
    }
}

/// Write `bytes` to `path`, owner read/write only on Unix from before the first byte (a leftover
/// file from a crash is narrowed too: the mode given at open only applies to new files).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path)?;
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    f.write_all(bytes)
}

thread_local! {
    /// While the main window draws: a dialog parented to it, so a dialog opened from it stays in
    /// front of it (a sheet on macOS). eframe gives other windows no handle: theirs have no parent.
    static PARENT: RefCell<Option<rfd::AsyncFileDialog>> = const { RefCell::new(None) };
}

/// Run the main window's frame `f` with the dialogs it opens parented to `frame`'s window.
pub fn parented<R>(frame: &eframe::Frame, f: impl FnOnce() -> R) -> R {
    PARENT.with(|p| p.replace(Some(rfd::AsyncFileDialog::new().set_parent(frame))));
    let r = f();
    PARENT.with(|p| p.replace(None));
    r
}

/// A dialog with `title`, parented to the main window while it draws.
fn dialog(title: &str) -> rfd::AsyncFileDialog {
    PARENT.with(|p| p.borrow().clone()).unwrap_or_default().set_title(title)
}

/// The dialog opens now; its future gives the path picked.
fn picked(pick: impl Future<Output = Option<rfd::FileHandle>> + 'static) -> DialogFuture {
    Box::pin(async move { pick.await.map(|f| f.path().to_path_buf()) })
}

/// How long a clipboard read may wait on the app that copied (Linux and the BSDs).
#[cfg(all(unix, not(target_os = "macos")))]
const CLIPBOARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Read the clipboard. On Linux and the BSDs the read runs on a helper thread with its own handle
/// and gives up after [`CLIPBOARD_TIMEOUT`]: a Wayland read waits on the app that copied with no
/// timeout of its own, so an app that hangs must not hang the window too.
#[cfg(all(unix, not(target_os = "macos")))]
fn read_clipboard<T: Send + 'static>(_: &mut arboard::Clipboard, read: fn(&mut arboard::Clipboard) -> Option<T>) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(arboard::Clipboard::new().ok().and_then(|mut c| read(&mut c)));
    });
    rx.recv_timeout(CLIPBOARD_TIMEOUT).ok().flatten()
}

/// Read the clipboard (the system's own reads don't wait on the app that copied).
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn read_clipboard<T>(c: &mut arboard::Clipboard, read: fn(&mut arboard::Clipboard) -> Option<T>) -> Option<T> {
    read(c)
}

/// Native Save as filters; other callers keep their existing combined filter.
fn save_filters<'a>(exts: &'a [&'a str]) -> Vec<(&'static str, &'a [&'a str])> {
    if exts == ["lzd", "fig"] {
        vec![("Lizard (.lzd)", &exts[..1]), (lizard_ui_egui::i18n::tr(".fig (experimental)"), &exts[1..])]
    } else {
        vec![("File", exts)]
    }
}

fn parallel_lead_instructions(count: usize, mode: lizard_ui_egui::agent_panel::ParallelMode) -> String {
    match mode {
        lizard_ui_egui::agent_panel::ParallelMode::Split => format!(
            "Parallel design run with {count} agents. You are the lead designer and own the complete result. First determine the number of screens the user explicitly requested. If no number is specified, produce ONE finished screen; a request for a web app is not permission to invent a second screen. Before spawning, create that single screen frame, establish its background, grid, typography, palette and shared navigation, then create non-overlapping section frames INSIDE it. Give other agents bounded sections, not separate screens, and build the remaining sections yourself at the same time. If multiple screens were explicitly requested, use one consistent design system across them. Call spawn_agents once with exactly {} config entries containing an existing section container and a distinct scoped task each. Each task must include concrete shared visual values (colours, type sizes, spacing, icon family), section bounds, and which global elements already exist. The child must use the established shell and must not add another page, screen, global navigation or brand treatment. Continue building your own section immediately after spawning; inspect the combined result, reconcile inconsistencies, and only then report completion. Never edit another agent's assigned section while it is working. If the work cannot be split into independent sections, explain why and continue alone.",
            count - 1
        ),
        lizard_ui_egui::agent_panel::ParallelMode::SideBySide => format!(
            "Parallel alternatives with {count} agents. You are the lead designer. Create exactly {count} separate root frames for complete versions of the same request. Call spawn_agents once with exactly {} config entries, each with a different root ID in containerNodes and a brief prompt asking for a complete independent version. Then make your own complete version inside the remaining root. Do not edit other agents' roots.",
            count - 1
        ),
    }
}

impl Services for Desktop {
    fn update_status(&mut self) -> UpdateStatus {
        #[cfg(target_os = "macos")]
        {
            self.updater.status()
        }
        #[cfg(not(target_os = "macos"))]
        {
            UpdateStatus::Unavailable
        }
    }
    fn update_check(&mut self) {
        #[cfg(target_os = "macos")]
        self.updater.check();
    }
    fn update_prepare(&mut self) {
        #[cfg(target_os = "macos")]
        self.updater.prepare();
    }
    fn update_install(&mut self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            self.updater.install()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Updates are unavailable on this platform".into())
        }
    }
    fn update_cleanup(&mut self) {
        #[cfg(target_os = "macos")]
        self.updater.cleanup();
    }
    fn agent_send(&mut self, kind: AgentKind, prompt: AgentPrompt, document: u64) -> Result<(), String> {
        let count = usize::from(prompt.agent_count.clamp(1, 6));
        if count > 1 {
            self.agent_reset();
        }
        if !self.agents.is_empty()
            && (self.agents.len() != count
                || self.agents.iter().any(|agent| agent.document != document || agent.kind != kind || agent.conversation != prompt.conversation))
        {
            self.agent_reset();
        }
        if self.agents.is_empty() {
            let cli = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|parent| parent.join(if cfg!(windows) { "lizard-cli.exe" } else { "lizard-cli" })))
                .filter(|p| p.is_file())
                .ok_or(tr("Lizard's agent bridge is missing. Reinstall Lizard or rebuild the app bundle with lizard-cli."))?
                .to_string_lossy()
                .into_owned();
            let cwd = self.dir.as_ref().map(|d| d.join("Agent")).unwrap_or_else(|| std::env::temp_dir().join("Lizard Agent"));
            std::fs::create_dir_all(&cwd).map_err(|error| trf("Could not prepare the agent folder: {error}", &[("error", &error)]))?;
            for index in 0..if count > 1 { 1 } else { count } {
                let port = match self.agent_port(document, index) {
                    Ok(port) => port,
                    Err(error) => {
                        self.agent_reset();
                        return Err(error);
                    }
                };
                match crate::agent::Agent::start(kind, document, prompt.conversation, port, &cwd.to_string_lossy(), &cli) {
                    Ok(agent) => self.agents.push(agent),
                    Err(error) => {
                        self.agent_reset();
                        return Err(error);
                    }
                }
            }
            if let Some((queue, context)) = &self.agent_control
                && let Some((port, enabled)) = crate::control_server::start_agent_reviewer(document, queue.clone(), context.clone())
            {
                self.agent_bridges.push((port, enabled.clone()));
                match crate::agent::Agent::start(kind, document, prompt.conversation, port, &cwd.to_string_lossy(), &cli) {
                    Ok(reviewer) => self.reviewer = Some(reviewer),
                    Err(error) => {
                        enabled.store(false, Ordering::Release);
                        log::warn!("Could not start design reviewer: {error}");
                    }
                }
            }
        }
        self.reviewer_template = Some(prompt.clone());
        for index in 0..self.agents.len() {
            let mut assigned = prompt.clone();
            if count > 1 {
                assigned.text.push_str("\n\n");
                assigned.text.push_str(&parallel_lead_instructions(count, prompt.parallel_mode));
                assigned.resume = prompt.resumes.get(index).cloned().flatten().or_else(|| prompt.resume.clone());
            }
            if let Err(error) = self.agents[index].send(assigned) {
                self.agent_reset();
                return Err(error);
            }
        }
        if count > 1 {
            self.pending_parallel = Some((kind, prompt, document));
        }
        Ok(())
    }

    fn agent_review(&mut self, document: u64, agent_index: usize, request: &str, roots: &[u64]) {
        if self.reviewer.as_ref().is_none_or(|reviewer| reviewer.document != document) {
            return;
        }
        if self.reviewer_busy {
            let pending = (document, agent_index, request.to_owned(), roots.to_vec());
            if let Some(existing) = self.reviewer_pending.iter_mut().find(|(_, index, _, _)| *index == agent_index) {
                *existing = pending;
            } else {
                self.reviewer_pending.push(pending);
            }
            return;
        }
        let Some(template) = self.reviewer_template.clone() else { return };
        let review = AgentPrompt {
            text: format!(
                "You are Lizard's design orchestrator. Review the current document while the designer keeps working. Use only the Lizard MCP inspect_document, get_node and screenshot tools. Your connection is read-only. Check the sections completed so far against this request: {request}. Review agent {agent_index}'s assigned root IDs {roots:?}; when this list is not empty, report issues only inside those roots. When it is empty, review the shared screen. Focus on accidental opaque structural frames, clipping, overlap, inconsistent colours/type/spacing and missing requested content. Ignore unfinished sections. Reply with at most three concrete issues including node IDs, or exactly 'No actionable issue yet.' Do not make edits or ask the designer to stop."
            ),
            images: Vec::new(),
            resume: None,
            resumes: Vec::new(),
            agent_count: 1,
            ..template
        };
        if let Some(reviewer) = &self.reviewer
            && reviewer.send(review).is_ok()
        {
            self.reviewer_busy = true;
            self.reviewer_agent_index = agent_index;
            self.reviewer_text.clear();
        }
    }

    fn agent_review_pending(&self) -> bool {
        self.reviewer_busy || !self.reviewer_pending.is_empty()
    }

    fn agent_spawn(&mut self, document: u64, config: &[Value]) -> Result<(), String> {
        let (kind, prompt, expected_document) = self.pending_parallel.clone().ok_or("No parallel run is waiting for agents")?;
        let count = usize::from(prompt.agent_count.clamp(1, 6));
        if document != expected_document || self.agents.len() != 1 || config.len() != count - 1 {
            return Err(format!("Expected {} assigned agents in the active document", count - 1));
        }
        let cli = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|parent| parent.join(if cfg!(windows) { "lizard-cli.exe" } else { "lizard-cli" })))
            .filter(|p| p.is_file())
            .ok_or("Lizard's agent bridge is missing")?
            .to_string_lossy()
            .into_owned();
        let cwd = self.dir.as_ref().map(|d| d.join("Agent")).unwrap_or_else(|| std::env::temp_dir().join("Lizard Agent"));
        let old_bridges = self.agent_bridges.len();
        let mut children = Vec::with_capacity(config.len());
        let prepared = (|| -> Result<(), String> {
            for (offset, task) in config.iter().enumerate() {
                let index = offset + 1;
                let port = self.agent_port(document, index)?;
                let agent = crate::agent::Agent::start(kind, document, prompt.conversation, port, &cwd.to_string_lossy(), &cli)?;
                let mut assigned = prompt.clone();
                let roots = task.get("containerNodes").and_then(Value::as_array).cloned().unwrap_or_default();
                let description = task
                    .get("prompt")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| prompt.text.rsplit_once("User request:\n").map_or(prompt.text.as_str(), |(_, request)| request));
                let scope = match prompt.parallel_mode {
                    lizard_ui_egui::agent_panel::ParallelMode::Split => {
                        "Build only your assigned section of the ONE shared design. The original request above supplies product context, not permission to build a second full screen. Do not create another root screen, page, navigation shell or visual theme. Match the lead's existing colours, type, spacing and icon family exactly; inspect surrounding work before and after editing. Finish by reporting your section and any visual inconsistency you noticed."
                    }
                    lizard_ui_egui::agent_panel::ParallelMode::SideBySide => {
                        "Build one complete independent alternative in your assigned root frame. Do not alter the other alternatives. Finish by reporting what distinguishes yours."
                    }
                };
                assigned.text.push_str(&format!("\n\nYou are collaborator {} of {}. Your exclusive task: {}. Work only inside the assigned frame IDs: {}. {} Do not move, delete, or edit another agent's assigned frame.\nAssigned frame snapshot: {}", index + 1, count, description, Value::Array(roots), scope, task.get("bootstrap").unwrap_or(&Value::Null)));
                assigned.resume = None;
                agent.send(assigned)?;
                children.push(agent);
            }
            Ok(())
        })();
        if let Err(error) = prepared {
            drop(children);
            for (_, enabled) in self.agent_bridges.drain(old_bridges..) {
                enabled.store(false, Ordering::Release);
            }
            return Err(error);
        }
        self.agents.extend(children);
        self.pending_parallel = None;
        Ok(())
    }

    fn agent_poll(&mut self) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let mut lead_finished_without_spawn = false;
        for (index, agent) in self.agents.iter().enumerate() {
            for event in agent.poll() {
                if index == 0 && matches!(event, AgentEvent::Done(_) | AgentEvent::Error(_)) && self.pending_parallel.is_some() {
                    lead_finished_without_spawn = true;
                }
                if self.agents.len() > 1 {
                    events.push(AgentEvent::Scoped { index, event: Box::new(event) });
                } else {
                    events.push(event);
                }
            }
        }
        if lead_finished_without_spawn && let Some((_, prompt, _)) = self.pending_parallel.take() {
            for index in 1..usize::from(prompt.agent_count.clamp(1, 6)) {
                events.push(AgentEvent::Scoped { index, event: Box::new(AgentEvent::Done("Not started".into())) });
            }
        }
        if let Some(reviewer) = &self.reviewer {
            for event in reviewer.poll() {
                match event {
                    AgentEvent::Text(chunk) if self.reviewer_text.len() < 8_000 => self.reviewer_text.push_str(&chunk),
                    AgentEvent::Done(_) => {
                        self.reviewer_busy = false;
                        let feedback = std::mem::take(&mut self.reviewer_text);
                        if !feedback.trim().is_empty() && !feedback.trim().eq_ignore_ascii_case("No actionable issue yet.") {
                            events.push(AgentEvent::ReviewFeedback { agent_index: self.reviewer_agent_index, text: feedback });
                        }
                    }
                    AgentEvent::Error(error) => {
                        self.reviewer_busy = false;
                        self.reviewer_text.clear();
                        log::warn!("Design review failed: {error}");
                    }
                    _ => {}
                }
            }
        }
        if !self.reviewer_busy && !self.reviewer_pending.is_empty() {
            let (document, agent_index, request, roots) = self.reviewer_pending.remove(0);
            self.agent_review(document, agent_index, &request, &roots);
        }
        if let Some(login) = &self.agent_login_events {
            for event in login.try_iter().take(20) {
                if matches!(event, AgentEvent::LoginComplete)
                    && let Some(kind) = self.agent_login_kind
                {
                    self.agent_signed_in[kind as usize] = true;
                    self.agent_auth_checked[kind as usize] = Some(true);
                }
                events.push(event);
            }
        }
        if let Some(models) = &self.agent_model_events {
            events.extend(models.try_iter().take(20));
        }
        events
    }

    fn agent_connection(&mut self, kind: AgentKind) -> lizard_ui_egui::agent_panel::AgentConnection {
        while let Ok((checked_kind, authenticated)) = self.agent_auth_probe_rx.try_recv() {
            if !self.agent_signed_in[checked_kind as usize] {
                self.agent_auth_checked[checked_kind as usize] = Some(authenticated);
            }
        }
        if !crate::agent::login_available(kind) {
            return lizard_ui_egui::agent_panel::AgentConnection { installed: false, authenticated: false, detail: String::new() };
        }
        if kind == AgentKind::Codex {
            let authenticated = crate::agent::codex_authenticated();
            lizard_ui_egui::agent_panel::AgentConnection { installed: true, authenticated, detail: String::new() }
        } else {
            let authenticated = if kind == AgentKind::Claude {
                crate::agent::claude_authenticated()
            } else {
                let index = kind as usize;
                if !self.agent_auth_probe_started[index] {
                    self.agent_auth_probe_started[index] = true;
                    let sender = self.agent_auth_probe_tx.clone();
                    if std::thread::Builder::new()
                        .name("lizard-agent-auth-check".into())
                        .spawn(move || {
                            let _ = sender.send((kind, crate::agent::acp_authenticated(kind)));
                        })
                        .is_err()
                    {
                        self.agent_auth_checked[index] = Some(false);
                    }
                }
                self.agent_signed_in[index] || self.agent_auth_checked[index].unwrap_or(false)
            };
            lizard_ui_egui::agent_panel::AgentConnection {
                installed: true,
                authenticated,
                detail: if kind != AgentKind::Claude && self.agent_auth_checked[kind as usize].is_none() && !self.agent_signed_in[kind as usize] {
                    "checking".into()
                } else {
                    String::new()
                },
            }
        }
    }

    fn agent_login(&mut self, kind: AgentKind) -> Result<(), String> {
        if let Some(previous) = self.agent_login_attempt.take() {
            previous.cancel();
        }
        let (send, receive) = std::sync::mpsc::channel();
        let attempt = crate::agent::LoginAttempt::default();
        crate::agent::login_provider(kind, send, attempt.clone())?;
        self.agent_login_events = Some(receive);
        self.agent_login_kind = Some(kind);
        self.agent_login_attempt = Some(attempt);
        Ok(())
    }

    fn agent_cancel_login(&mut self) {
        if let Some(attempt) = self.agent_login_attempt.take() {
            attempt.cancel();
        }
        self.agent_login_events = None;
        self.agent_login_kind = None;
    }

    fn agent_refresh_connection(&mut self, kind: AgentKind) {
        let index = kind as usize;
        self.agent_auth_checked[index] = None;
        self.agent_auth_probe_started[index] = false;
        self.agent_signed_in[index] = false;
    }

    fn agent_request_models(&mut self, kind: AgentKind) {
        if kind == AgentKind::Codex {
            let (send, receive) = std::sync::mpsc::channel();
            if let Err(error) = crate::agent::request_codex_models(send) {
                log::warn!("codex models: {error}");
            } else {
                self.agent_model_events = Some(receive);
            }
        }
    }

    fn agent_stop(&mut self) {
        for agent in &self.agents {
            agent.stop();
        }
        if let Some(reviewer) = &self.reviewer {
            reviewer.stop();
        }
    }

    fn agent_reset(&mut self) {
        self.agents.clear();
        self.reviewer = None;
        self.reviewer_busy = false;
        self.reviewer_agent_index = 0;
        self.reviewer_text.clear();
        self.reviewer_pending.clear();
        self.reviewer_template = None;
        self.pending_parallel = None;
        for (_, enabled) in self.agent_bridges.drain(..) {
            enabled.store(false, Ordering::Release);
        }
    }

    fn agent_permission(&mut self, id: Value, option: Option<String>) {
        let index = id.get("agent_index").and_then(Value::as_u64).and_then(|value| usize::try_from(value).ok()).unwrap_or(0);
        let request = id.get("request").cloned().unwrap_or(id);
        if let Some(agent) = self.agents.get(index) {
            agent.permission(request, option);
        }
    }

    fn agent_history_load(&mut self) -> Value {
        self.dir
            .as_ref()
            .and_then(|dir| std::fs::read(dir.join("agent-chats.json")).ok())
            .filter(|bytes| bytes.len() <= 8 * 1024 * 1024)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null)
    }

    fn agent_history_save(&mut self, history: &Value) {
        if let Some(dir) = &self.dir
            && let Ok(bytes) = serde_json::to_vec(history)
            && bytes.len() <= 8 * 1024 * 1024
            && let Err(error) = std::fs::create_dir_all(dir)
                .and_then(|()| write_private(&dir.join("agent-chats.json.tmp"), &bytes))
                .and_then(|()| std::fs::rename(dir.join("agent-chats.json.tmp"), dir.join("agent-chats.json")))
        {
            log::warn!("agent chat history: {error}");
        }
    }
    fn pick_open(&mut self, title: &str, exts: &[&str]) -> DialogFuture {
        picked(dialog(title).add_filter("Supported files", exts).pick_file())
    }
    fn pick_save(&mut self, title: &str, name: &str, exts: &[&str]) -> DialogFuture {
        let mut dialog = dialog(title).set_file_name(name);
        for (label, extensions) in save_filters(exts) {
            dialog = dialog.add_filter(label, extensions);
        }
        picked(dialog.save_file())
    }
    fn pick_folder(&mut self, title: &str) -> DialogFuture {
        picked(dialog(title).pick_folder())
    }
    fn clipboard_set_text(&mut self, s: &str) {
        if let Some(c) = self.clipboard.as_mut() {
            let _ = c.set_text(s.to_string());
        }
    }
    fn clipboard_get_text(&mut self) -> Option<String> {
        read_clipboard(self.clipboard.as_mut()?, |c| c.get_text().ok())
    }
    fn clipboard_get_html(&mut self) -> Option<String> {
        read_clipboard(self.clipboard.as_mut()?, |c| c.get().html().ok())
    }
    fn clipboard_set_png(&mut self, w: u32, h: u32, rgba: &[u8]) {
        if let Some(c) = self.clipboard.as_mut() {
            let _ = c.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(rgba) });
        }
    }
    fn clipboard_get_image(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let img = read_clipboard(self.clipboard.as_mut()?, |c| c.get_image().ok())?;
        let (w, h) = (u32::try_from(img.width).ok()?, u32::try_from(img.height).ok()?);
        // Refuse absurd sizes (and mismatched buffers) rather than allocate.
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 100_000_000 || img.bytes.len() != (w as usize) * (h as usize) * 4 {
            return None;
        }
        Some((w, h, img.bytes.into_owned()))
    }
    fn open_url(&mut self, url: &str) {
        let _ = webbrowser::open(url);
    }
    fn reveal_path(&mut self, path: &Path) {
        if let Err(e) = reveal(path) {
            log::warn!("show in folder: {}: {e}", path.display());
        }
    }
    fn save_prefs(&mut self, prefs: &Prefs) {
        if let Some(dir) = &self.dir {
            write_prefs(dir, prefs);
        }
    }
    fn emergency_save(&mut self, session: &Session) {
        if let Some(r) = crate::recovery::lock(&self.recovery).as_mut() {
            r.write(session, true);
            // Earlier runs' copies now covered go, so the next start doesn't open them twice.
            r.settle(session);
        }
    }
    fn reduce_motion(&self) -> bool {
        self.reduce_motion.load(Ordering::Relaxed)
    }
    fn gamepad_held(&mut self) -> u32 {
        crate::gamepad::held()
    }
    fn system_language(&self) -> Option<String> {
        self.language.clone()
    }
}

/// The window icon from the bundled app icon (if present).
pub fn window_icon() -> Option<egui::IconData> {
    // The macOS icon includes its own rounded tile and transparent Dock margin.
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../../../assets/app-icon/lizard-256.png");
    #[cfg(not(target_os = "macos"))]
    let bytes = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/com.brava.lizard.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

#[cfg(test)]
mod save_filter_tests {
    use super::save_filters;

    #[test]
    fn native_and_fig_save_filters_are_separate_with_native_first() {
        let extensions = ["lzd", "fig"];
        assert_eq!(
            save_filters(&extensions),
            vec![("Lizard (.lzd)", &extensions[..1]), (lizard_ui_egui::i18n::tr(".fig (experimental)"), &extensions[1..])]
        );
    }

    #[test]
    fn other_save_filters_keep_one_combined_group() {
        for extensions in [&["lizard"][..], &["fig"][..], &["png"][..], &["png", "jpg", "webp"][..], &[][..]] {
            assert_eq!(save_filters(extensions), vec![("File", extensions)]);
        }
    }
}
