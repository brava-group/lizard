//! The Agent sidebar and its conversation state.

use base64::Engine as _;
use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use lizard_doc::NodeId;
use serde::{Deserialize, Serialize};

use crate::LizardApp;
use crate::a11y;
use crate::icons::{self, AgentIcon, LayerIcon};
use crate::primitives::{Menu, MenuItem, ShimmerDirection, ShimmerText};
use crate::theme::{Tokens, regular, semibold};

const BLUE: Color32 = Color32::from_rgb(0x7C, 0xC4, 0xF8);
const BLUE_BG: Color32 = Color32::from_rgb(0x39, 0x43, 0x60);
const MUTED: Color32 = Color32::from_rgba_premultiplied(178, 178, 178, 178);
const DIM: Color32 = Color32::from_rgba_premultiplied(102, 102, 102, 102);
const CHAT_BUBBLE: Color32 = Color32::from_rgb(0x40, 0x40, 0x40);
const RECEIPT_BG: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
const RECEIPT_BORDER: Color32 = Color32::from_rgb(0x3D, 0x3D, 0x42);
const RECEIPT_GREEN: Color32 = Color32::from_rgb(0x00, 0xD4, 0x82);
const TRANSCRIPT_SCROLL_GUTTER: f32 = 16.0;
const WORK_WORDS: [&str; 7] = ["Pensando…", "Trabalhando…", "Desenhando…", "Criando…", "Revisando…", "Validando…", "Testando…"];

#[derive(Clone)]
struct WorkWordState {
    index: usize,
    next_change: f64,
    seed: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParallelMode {
    #[default]
    Split,
    SideBySide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VariantMode {
    Layout,
    #[default]
    Style,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    Codex,
    Claude,
    Gemini,
    Kimi,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Gemini => "Gemini CLI",
            Self::Kimi => "Kimi Code",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentConnection {
    pub installed: bool,
    pub authenticated: bool,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub enum AgentEvent {
    Ready,
    Session(String),
    LoginUrl(String),
    LoginComplete,
    LoginError(String),
    Models(Vec<(String, String)>),
    Text(String),
    Thinking(String),
    Tool {
        id: String,
        title: Option<String>,
        status: Option<String>,
        input: Option<Option<serde_json::Value>>,
        output: Option<Option<serde_json::Value>>,
        content: Option<Vec<serde_json::Value>>,
    },
    Done(String),
    Error(String),
    Permission {
        id: serde_json::Value,
        title: String,
        options: Vec<(String, String)>,
    },
    Scoped {
        index: usize,
        event: Box<AgentEvent>,
    },
}

#[derive(Clone, Debug)]
pub struct AgentPermission {
    pub id: serde_json::Value,
    pub title: String,
    pub options: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub model_label: String,
    #[serde(default)]
    pub agent_index: usize,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub variant_index: u8,
    #[serde(default)]
    pub variant_total: u8,
    #[serde(default)]
    pub thinking: String,
    #[serde(skip)]
    pub previews: Vec<AgentPreview>,
    #[serde(default)]
    pub selected_nodes: Vec<ChatNodeRef>,
    #[serde(default)]
    pub attached_images: Vec<String>,
    #[serde(default)]
    pub tools: Vec<ChatTool>,
    #[serde(default)]
    pub work_seconds: Option<u64>,
    #[serde(default)]
    pub stopped: bool,
    #[serde(default)]
    pub undo_before: usize,
    #[serde(default)]
    pub undo_after: usize,
    #[serde(default)]
    pub revision_after: u64,
    #[serde(default)]
    pub reverted: bool,
}

#[derive(Clone)]
pub struct AgentPreview {
    pub data: String,
    pub texture: Option<egui::TextureHandle>,
}

impl std::fmt::Debug for AgentPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPreview").field("bytes", &self.data.len()).finish()
    }
}

#[derive(Clone, Debug)]
pub struct AgentPulse {
    pub id: NodeId,
    pub started: web_time::Instant,
    pub inspection: bool,
}

#[derive(Clone, Debug)]
pub struct AgentShimmer {
    pub id: NodeId,
    pub started: web_time::Instant,
    pub ongoing: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AgentPresence {
    pub document: Option<u64>,
    pub conversation: Option<u64>,
    pub target: Option<NodeId>,
    /// Smoothed cursor position in page coordinates; camera changes must not move it off target.
    pub shown: Option<(f64, f64)>,
    pub updated: Option<web_time::Instant>,
    pub pulses: Vec<AgentPulse>,
    pub shimmers: Vec<AgentShimmer>,
    pub active: bool,
    pub generating: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatNodeRef {
    pub id: NodeId,
    pub name: String,
}

/// The ACP prompt keeps binary images separate from the visible text and node references.
#[derive(Clone)]
pub struct AgentPrompt {
    pub text: String,
    pub images: Vec<AgentImage>,
    pub conversation: u64,
    pub resume: Option<String>,
    pub resumes: Vec<Option<String>>,
    pub model: Option<String>,
    pub agent_count: u8,
    pub parallel_mode: ParallelMode,
    pub variant_count: u8,
    pub variant_mode: VariantMode,
}

#[derive(Clone)]
pub struct AgentImage {
    pub mime: String,
    pub data: String,
}

pub struct DraftImage {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub preview: Option<egui::TextureHandle>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChatTool {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub content: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Conversation {
    pub id: u64,
    pub document: String,
    pub title: String,
    pub agent: AgentKind,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub remote_session: Option<String>,
    #[serde(default)]
    pub remote_sessions: Vec<Option<String>>,
}

#[derive(Default)]
pub struct AgentUi {
    pub presence: AgentPresence,
    pub draft: String,
    pub images: Vec<DraftImage>,
    pub pending_images: Vec<DraftImage>,
    /// IDs hidden from this draft's context, without changing the canvas selection.
    pub excluded_selection: Vec<NodeId>,
    pub observed_selection: Vec<NodeId>,
    pub observed_document: Option<u64>,
    pub turn_started: Option<web_time::Instant>,
    pub remaining_agents: u8,
    pub finished_agents: Vec<bool>,
    pub active_agent_count: u8,
    pub remaining_variants: u8,
    pub active_variant_count: u8,
    pub active_variant_mode: VariantMode,
    pub active_parallel_mode: ParallelMode,
    pub active_model: Option<String>,
    pub active_prompt: String,
    pub round_failed: bool,
    pub history_open: bool,
    pub selected_suggestion: Option<usize>,
    pub agent: AgentKind,
    pub conversations: Vec<Conversation>,
    pub active: Option<u64>,
    pub busy: bool,
    pub history_loaded: bool,
    pub runtime_document: Option<u64>,
    pub runtime_conversation: Option<u64>,
    pub permission: Option<AgentPermission>,
    pub connection: Option<(AgentKind, AgentConnection)>,
    pub login_pending: bool,
    pub model: Option<String>,
    pub models: Vec<(String, String)>,
    pub agent_count: u8,
    pub parallel_mode: ParallelMode,
    pub variant_count: u8,
    pub variant_mode: VariantMode,
    pub models_requested: Option<AgentKind>,
    pub models_loading_until: Option<web_time::Instant>,
}

fn document_key(app: &LizardApp) -> String {
    app.session.ds().ok().map_or_else(|| "sem-documento".to_string(), |ds| ds.path.clone().unwrap_or_else(|| format!("untitled:{}", ds.uid)))
}

fn save_history(app: &mut LizardApp) {
    if let Ok(value) = serde_json::to_value(&app.ui.agent.conversations) {
        app.services.agent_history_save(&value);
    }
}

fn agent_message(chat: &mut Conversation, index: usize) -> Option<&mut ChatMessage> {
    chat.messages.iter_mut().rev().find(|message| message.role == "assistant" && message.agent_index == index)
}

pub fn sync(app: &mut LizardApp, ctx: &egui::Context) {
    if !app.ui.agent.history_loaded {
        app.ui.agent.history_loaded = true;
        if let Ok(mut chats) = serde_json::from_value::<Vec<Conversation>>(app.services.agent_history_load()) {
            chats.truncate(100);
            for chat in &mut chats {
                chat.remote_sessions.truncate(6);
                chat.messages.truncate(200);
                for message in &mut chat.messages {
                    message.text.truncate(message.text.floor_char_boundary(64_000));
                    message.thinking.truncate(message.thinking.floor_char_boundary(32_000));
                }
            }
            app.ui.agent.conversations = chats;
        }
    }
    let document = app.session.ds().ok().map(|ds| ds.uid);
    if app.ui.agent.observed_document != document {
        app.ui.agent.presence = AgentPresence::default();
        app.ui.agent.observed_document = document;
        app.ui.agent.observed_selection.clear();
        app.ui.agent.excluded_selection.clear();
        app.ui.agent.images.clear();
    }
    let selection = app.session.ds().ok().map_or_else(Vec::new, |ds| ds.selection.clone());
    if app.ui.agent.observed_selection != selection {
        app.ui.agent.observed_selection = selection;
        app.ui.agent.excluded_selection.clear();
    }
    let key = document_key(app);
    if app.ui.agent.runtime_document == document
        && let Some(chat) = app.ui.agent.runtime_conversation.and_then(|id| app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id))
        && chat.document != key
    {
        chat.document = key.clone();
        save_history(app);
    }
    if app.ui.agent.active.is_some_and(|id| app.ui.agent.conversations.iter().any(|chat| chat.id == id && chat.document != key)) {
        app.ui.agent.active = None;
    }
    if app.ui.agent.runtime_document.is_some() && app.ui.agent.runtime_document != document {
        stop_chat(app);
    }
    let events = app.services.agent_poll();
    let mut completed = false;
    let mut next_variant = false;
    for event in events {
        let (agent_index, event) = match event {
            AgentEvent::Scoped { index, event } => (index, *event),
            event => (0, event),
        };
        let active = app.ui.agent.runtime_conversation.and_then(|id| app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id));
        match event {
            AgentEvent::Ready => {}
            AgentEvent::Session(session) => {
                if let Some(chat) = active {
                    if agent_index < 6 {
                        if chat.remote_sessions.len() <= agent_index {
                            chat.remote_sessions.resize(agent_index + 1, None);
                        }
                        chat.remote_sessions[agent_index] = Some(session.clone());
                    }
                    if agent_index == 0 {
                        chat.remote_session = Some(session);
                    }
                    completed = true;
                }
            }
            AgentEvent::LoginUrl(url) => {
                app.services.open_url(&url);
            }
            AgentEvent::LoginComplete => {
                app.ui.agent.login_pending = false;
                app.ui.agent.connection = None;
            }
            AgentEvent::LoginError(error) => {
                app.ui.agent.login_pending = false;
                app.toast(error);
            }
            AgentEvent::Models(models) => {
                app.ui.agent.models = models;
                app.ui.agent.models_loading_until = None;
            }
            AgentEvent::Text(chunk) => {
                if let Some(message) = active.and_then(|chat| agent_message(chat, agent_index))
                    && message.text.len() < 100_000
                {
                    message.text.push_str(&chunk);
                }
            }
            AgentEvent::Thinking(chunk) => {
                if let Some(message) = active.and_then(|chat| agent_message(chat, agent_index))
                    && message.thinking.len() < 32_000
                {
                    message.thinking.push_str(&chunk);
                }
            }
            AgentEvent::Tool { id, title, status, input, output, content } => {
                if let Some(message) = active.and_then(|chat| agent_message(chat, agent_index)) {
                    if let Some(tool) = message.tools.iter_mut().find(|tool| tool.id == id) {
                        if let Some(title) = title {
                            tool.title = title;
                        }
                        if let Some(status) = status {
                            tool.status = status;
                        }
                        if let Some(input) = input {
                            tool.input = input;
                        }
                        if let Some(output) = output {
                            tool.output = output;
                        }
                        if let Some(content) = content {
                            tool.content = content;
                        }
                    } else if message.tools.len() < 100 {
                        message.tools.push(ChatTool {
                            id,
                            title: title.unwrap_or_else(|| "Ação do agente".into()),
                            status: status.unwrap_or_else(|| "in_progress".into()),
                            input: input.flatten(),
                            output: output.flatten(),
                            content: content.unwrap_or_default(),
                        });
                    }
                }
            }
            AgentEvent::Permission { id, title, options } => {
                let id = if app.ui.agent.active_agent_count > 1 { serde_json::json!({"agent_index":agent_index,"request":id}) } else { id };
                app.ui.agent.permission = Some(AgentPermission { id, title, options });
            }
            AgentEvent::Done(reason) => {
                if !app.ui.agent.busy
                    || agent_index >= usize::from(app.ui.agent.active_agent_count)
                    || app.ui.agent.finished_agents.get(agent_index) == Some(&true)
                {
                    continue;
                }
                if let Some(finished) = app.ui.agent.finished_agents.get_mut(agent_index) {
                    *finished = true;
                }
                if let Some(message) = active.and_then(|chat| agent_message(chat, agent_index)) {
                    message.work_seconds = app.ui.agent.turn_started.as_ref().map(|started| started.elapsed().as_secs());
                    if let Ok(ds) = app.session.ds() {
                        message.undo_after = ds.history.undo.len();
                        message.revision_after = ds.revision;
                    }
                    message.stopped = reason != "end_turn";
                }
                app.ui.agent.remaining_agents = app.ui.agent.remaining_agents.saturating_sub(1);
                if reason != "end_turn" {
                    app.ui.agent.round_failed = true;
                }
                if app.ui.agent.remaining_agents == 0 {
                    if app.ui.agent.remaining_variants > 1 && !app.ui.agent.round_failed {
                        next_variant = true;
                    } else {
                        app.ui.agent.busy = false;
                        app.ui.agent.turn_started = None;
                        app.ui.agent.presence.active = false;
                        app.ui.agent.presence.updated = Some(web_time::Instant::now());
                        app.ui.agent.pending_images.clear();
                    }
                }
                completed = true;
            }
            AgentEvent::Error(error) => {
                if !app.ui.agent.busy
                    || agent_index >= usize::from(app.ui.agent.active_agent_count)
                    || app.ui.agent.finished_agents.get(agent_index) == Some(&true)
                {
                    continue;
                }
                if let Some(finished) = app.ui.agent.finished_agents.get_mut(agent_index) {
                    *finished = true;
                }
                if let Some(message) = active.and_then(|chat| agent_message(chat, agent_index)) {
                    message.work_seconds = app.ui.agent.turn_started.as_ref().map(|started| started.elapsed().as_secs());
                    message.stopped = true;
                    if !message.text.is_empty() {
                        message.text.push_str("\n\n");
                    }
                    message.text.push_str(&format!("Erro: {error}"));
                } else {
                    app.toast(error);
                }
                app.ui.agent.round_failed = true;
                app.ui.agent.remaining_agents = app.ui.agent.remaining_agents.saturating_sub(1);
                if app.ui.agent.remaining_agents == 0 {
                    app.ui.agent.presence.active = false;
                    app.ui.agent.presence.updated = Some(web_time::Instant::now());
                    app.ui.agent.turn_started = None;
                    if app.session.ds().ok().is_some_and(|ds| Some(ds.uid) == app.ui.agent.runtime_document) {
                        app.ui.agent.images.append(&mut app.ui.agent.pending_images);
                    } else {
                        app.ui.agent.pending_images.clear();
                    }
                    app.ui.agent.busy = false;
                    app.services.agent_reset();
                    app.ui.agent.runtime_document = None;
                    app.ui.agent.runtime_conversation = None;
                }
                completed = true;
            }
            AgentEvent::Scoped { .. } => {}
        }
    }
    if next_variant {
        begin_next_variant(app);
        completed = true;
    }
    for chat in &mut app.ui.agent.conversations {
        for message in &mut chat.messages {
            for (index, preview) in message.previews.iter_mut().enumerate() {
                if preview.texture.is_none()
                    && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&preview.data)
                    && let Ok(image) = image::load_from_memory(&bytes)
                {
                    let rgba = image.thumbnail(320, 240).to_rgba8();
                    let color = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
                    preview.texture = Some(ctx.load_texture(format!("agent-preview-{}-{index}", chat.id), color, egui::TextureOptions::LINEAR));
                    preview.data.clear();
                }
            }
        }
    }
    if completed {
        save_history(app);
    }
    if app.ui.agent.busy || app.ui.agent.login_pending || app.ui.agent.models_loading_until.is_some_and(|until| until > web_time::Instant::now()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn selected_context(app: &LizardApp) -> Vec<ChatNodeRef> {
    let Ok(ds) = app.session.ds() else { return Vec::new() };
    ds.selection
        .iter()
        .filter(|id| !app.ui.agent.excluded_selection.contains(id))
        .filter_map(|id| ds.doc.node(*id).map(|node| ChatNodeRef { id: *id, name: node.name.clone() }))
        .take(32)
        .collect()
}

const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

fn attach_image(app: &mut LizardApp, name: String, bytes: Vec<u8>) -> Result<(), String> {
    if app.ui.agent.images.len() >= 4
        || bytes.is_empty()
        || bytes.len().saturating_add(app.ui.agent.images.iter().map(|image| image.bytes.len()).sum::<usize>()) > MAX_IMAGE_BYTES
    {
        return Err("Anexe até 4 imagens, somando no máximo 4 MB.".into());
    }
    let mime = match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        _ => return Err("Use uma imagem PNG ou JPEG.".into()),
    };
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    if width == 0 || height == 0 || width > 4096 || height > 4096 || u64::from(width) * u64::from(height) > 12_000_000 {
        return Err("A imagem deve ter até 4.096 px por lado e 12 milhões de pixels.".into());
    }
    image::load_from_memory(&bytes).map_err(|error| format!("A imagem está danificada: {error}"))?;
    app.ui.agent.images.push(DraftImage { name, mime: mime.into(), bytes, preview: None });
    Ok(())
}

fn attach_rgba_image(app: &mut LizardApp, width: u32, height: u32, rgba: Vec<u8>) {
    let Some(expected) = usize::try_from(width).ok().and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)?.checked_mul(4))) else {
        app.toast("A imagem da área de transferência é grande demais.");
        return;
    };
    if rgba.len() != expected || width > 4096 || height > 4096 || u64::from(width) * u64::from(height) > 12_000_000 {
        app.toast("A imagem da área de transferência é grande demais.");
        return;
    }
    let Some(image) = image::RgbaImage::from_raw(width, height, rgba) else {
        app.toast("Não foi possível ler a imagem copiada.");
        return;
    };
    let mut out = std::io::Cursor::new(Vec::new());
    match image::DynamicImage::ImageRgba8(image).write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => {
            if let Err(error) = attach_image(app, "Imagem colada.png".into(), out.into_inner()) {
                app.toast(error);
            }
        }
        Err(error) => app.toast(error.to_string()),
    }
}

fn attach_clipboard_image(app: &mut LizardApp) {
    let Some((width, height, rgba)) = app.services.clipboard_get_image() else {
        app.toast("Não há uma imagem na área de transferência.");
        return;
    };
    attach_rgba_image(app, width, height, rgba);
}

fn paste_image_into_composer(app: &mut LizardApp, ui: &Ui) {
    if !ui.memory(|memory| memory.has_focus(Id::new("agent_prompt"))) {
        return;
    }
    let paste_requested = ui.input(|input| {
        input.events.iter().any(|event| {
            matches!(event, egui::Event::Paste(_))
                || matches!(event, egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, repeat: false, .. } if modifiers.command && !modifiers.shift && !modifiers.alt)
        })
    });
    if !paste_requested {
        return;
    }
    // A copied design may include a PNG preview. Keep that paste on the design path; ordinary
    // HTML or text alongside a real image does not turn the image into text in the composer.
    if app.services.clipboard_get_html().is_some_and(|html| lizard_engine::cmd::edit::design_clipboard_buffer(&html).is_some()) {
        return;
    }
    let Some((width, height, rgba)) = app.services.clipboard_get_image() else {
        return;
    };
    // egui also receives a Paste event for image clipboards on macOS, sometimes containing alt
    // text. Consume it before TextEdit and the global canvas shortcuts see the same events.
    ui.ctx().input_mut(|input| {
        input.events.retain(|event| {
            !matches!(event, egui::Event::Paste(_))
                && !matches!(event, egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, repeat: false, .. } if modifiers.command && !modifiers.shift && !modifiers.alt)
        });
    });
    attach_rgba_image(app, width, height, rgba);
}

fn send_prompt(app: &mut LizardApp) {
    let prompt = app.ui.agent.draft.trim().to_string();
    let selected_nodes = selected_context(app);
    if (prompt.is_empty() && selected_nodes.is_empty() && app.ui.agent.images.is_empty()) || app.ui.agent.busy {
        return;
    }
    let Ok(ds) = app.session.ds() else {
        app.toast("Abra um documento para conversar com o agente.");
        return;
    };
    let document = ds.uid;
    let undo_before = ds.history.undo.len();
    let key = document_key(app);
    let active = app.ui.agent.active.filter(|id| app.ui.agent.conversations.iter().any(|chat| chat.id == *id && chat.document == key));
    let id = active.unwrap_or_else(|| web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(1, |time| time.as_nanos() as u64));
    let resume = app.ui.agent.conversations.iter().find(|chat| chat.id == id).and_then(|chat| chat.remote_session.clone());
    let resumes = app.ui.agent.conversations.iter().find(|chat| chat.id == id).map_or_else(Vec::new, |chat| chat.remote_sessions.clone());
    let prior = if app.ui.agent.runtime_conversation == Some(id) || (app.ui.agent.agent == AgentKind::Codex && resume.is_some()) {
        String::new()
    } else {
        app.ui.agent.conversations.iter().find(|chat| chat.id == id).map_or_else(String::new, |chat| {
            chat.messages
                .iter()
                .rev()
                .take(12)
                .rev()
                .map(|message| format!("{}: {}", message.role, message.text.chars().take(3_000).collect::<String>()))
                .collect::<Vec<_>>()
                .join("\n")
        })
    };
    let new_local_session = app.ui.agent.runtime_conversation != Some(id);
    let context = crate::agent_harness::build_turn(ds, &selected_nodes, &prompt, &prior, new_local_session);
    let images = app
        .ui
        .agent
        .images
        .iter()
        .map(|image| AgentImage { mime: image.mime.clone(), data: base64::engine::general_purpose::STANDARD.encode(&image.bytes) })
        .collect();
    let attached_images = app.ui.agent.images.iter().map(|image| image.name.clone()).collect();
    let first_selected_node = selected_nodes.first().map(|node| node.id);
    let model_label = app
        .ui
        .agent
        .model
        .as_deref()
        .and_then(|model| app.ui.agent.models.iter().find(|(id, _)| id == model).map(|(_, label)| label.clone()))
        .unwrap_or_else(|| app.ui.agent.agent.label().to_string());
    let created_at = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
    let agent_count = app.ui.agent.agent_count.clamp(1, 6);
    let variant_count = app.ui.agent.variant_count.clamp(1, 6);
    match app.services.agent_send(
        app.ui.agent.agent,
        AgentPrompt {
            text: context,
            images,
            conversation: id,
            resume,
            resumes,
            model: app.ui.agent.model.clone(),
            agent_count,
            parallel_mode: app.ui.agent.parallel_mode,
            variant_count,
            variant_mode: app.ui.agent.variant_mode,
        },
        document,
    ) {
        Ok(()) => {
            if active.is_none() {
                app.ui.agent.conversations.push(Conversation {
                    id,
                    document: key,
                    title: if prompt.is_empty() {
                        selected_nodes.first().map_or_else(
                            || app.ui.agent.images.first().map_or_else(|| "Novo chat".to_string(), |image| image.name.clone()),
                            |node| node.name.clone(),
                        )
                    } else {
                        prompt.chars().take(42).collect()
                    },
                    agent: app.ui.agent.agent,
                    messages: Vec::new(),
                    remote_session: None,
                    remote_sessions: Vec::new(),
                });
            }
            if let Some(chat) = app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id) {
                chat.messages.push(ChatMessage {
                    role: "user".into(),
                    text: prompt,
                    created_at,
                    selected_nodes,
                    attached_images,
                    ..Default::default()
                });
                for index in 0..usize::from(agent_count) {
                    chat.messages.push(ChatMessage {
                        role: "assistant".into(),
                        model_label: model_label.clone(),
                        created_at,
                        agent_index: index,
                        parallel: agent_count > 1,
                        variant_index: 1,
                        variant_total: variant_count,
                        undo_before,
                        ..Default::default()
                    });
                }
            }
            app.ui.agent.active = Some(id);
            app.ui.agent.active_agent_count = agent_count;
            app.ui.agent.remaining_agents = agent_count;
            app.ui.agent.finished_agents = vec![false; usize::from(agent_count)];
            app.ui.agent.remaining_variants = variant_count;
            app.ui.agent.active_variant_count = variant_count;
            app.ui.agent.active_variant_mode = app.ui.agent.variant_mode;
            app.ui.agent.active_parallel_mode = app.ui.agent.parallel_mode;
            app.ui.agent.active_model = app.ui.agent.model.clone();
            app.ui.agent.active_prompt = app.ui.agent.draft.clone();
            app.ui.agent.round_failed = false;
            app.ui.agent.runtime_conversation = Some(id);
            app.ui.agent.runtime_document = Some(document);
            app.ui.agent.draft.clear();
            app.ui.agent.pending_images = std::mem::take(&mut app.ui.agent.images);
            app.ui.agent.turn_started = Some(web_time::Instant::now());
            app.ui.agent.busy = true;
            app.ui.agent.presence.document = Some(document);
            app.ui.agent.presence.conversation = Some(id);
            app.ui.agent.presence.target = first_selected_node;
            app.ui.agent.presence.shown = None;
            app.ui.agent.presence.pulses.clear();
            app.ui.agent.presence.shimmers.clear();
            app.ui.agent.presence.generating = false;
            app.ui.agent.presence.active = true;
            app.ui.agent.presence.updated = Some(web_time::Instant::now());
            save_history(app);
        }
        Err(error) => {
            app.services.agent_reset();
            app.toast(error);
        }
    }
}

fn begin_next_variant(app: &mut LizardApp) {
    let Some(conversation) = app.ui.agent.runtime_conversation else { return };
    let Ok(ds) = app.session.ds() else {
        app.ui.agent.busy = false;
        return;
    };
    let document = ds.uid;
    let undo_before = ds.history.undo.len();
    let direction = match app.ui.agent.active_variant_mode {
        VariantMode::Layout => "Explore uma composição diferente, preservando a linguagem visual.",
        VariantMode::Style => "Explore uma direção visual diferente para o mesmo pedido.",
    };
    let instruction = format!(
        "Crie uma nova variação independente da proposta anterior. {direction} Mantenha o resultado anterior para comparação, em vez de o substituir. Pedido original: {}",
        app.ui.agent.active_prompt
    );
    let selected_nodes = app
        .ui
        .agent
        .conversations
        .iter()
        .find(|chat| chat.id == conversation)
        .and_then(|chat| chat.messages.iter().rev().find(|message| message.role == "user"))
        .map_or_else(Vec::new, |message| message.selected_nodes.clone());
    let context = crate::agent_harness::build_turn(ds, &selected_nodes, &instruction, "", true);
    let images = app
        .ui
        .agent
        .pending_images
        .iter()
        .map(|image| AgentImage { mime: image.mime.clone(), data: base64::engine::general_purpose::STANDARD.encode(&image.bytes) })
        .collect();
    let agent_count = app.ui.agent.active_agent_count.clamp(1, 6);
    let prompt = AgentPrompt {
        text: context,
        images,
        conversation,
        resume: None,
        resumes: Vec::new(),
        model: app.ui.agent.active_model.clone(),
        agent_count,
        parallel_mode: app.ui.agent.active_parallel_mode,
        variant_count: 1,
        variant_mode: app.ui.agent.active_variant_mode,
    };
    if let Err(error) = app.services.agent_send(app.ui.agent.agent, prompt, document) {
        app.toast(error);
        app.ui.agent.busy = false;
        app.ui.agent.round_failed = true;
        app.ui.agent.remaining_agents = 0;
        app.ui.agent.finished_agents.clear();
        app.ui.agent.remaining_variants = 0;
        app.ui.agent.images.append(&mut app.ui.agent.pending_images);
        app.ui.agent.runtime_document = None;
        app.ui.agent.runtime_conversation = None;
        app.services.agent_reset();
        return;
    }
    let created_at = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
    let model_label = app
        .ui
        .agent
        .active_model
        .as_deref()
        .and_then(|model| app.ui.agent.models.iter().find(|(id, _)| id == model).map(|(_, label)| label.clone()))
        .unwrap_or_else(|| app.ui.agent.agent.label().to_string());
    if let Some(chat) = app.ui.agent.conversations.iter_mut().find(|chat| chat.id == conversation) {
        chat.remote_session = None;
        chat.remote_sessions.clear();
        for index in 0..usize::from(agent_count) {
            chat.messages.push(ChatMessage {
                role: "assistant".into(),
                model_label: model_label.clone(),
                created_at,
                agent_index: index,
                parallel: agent_count > 1,
                variant_index: app.ui.agent.active_variant_count.saturating_sub(app.ui.agent.remaining_variants).saturating_add(2),
                variant_total: app.ui.agent.active_variant_count,
                undo_before,
                ..Default::default()
            });
        }
    }
    app.ui.agent.remaining_variants = app.ui.agent.remaining_variants.saturating_sub(1);
    app.ui.agent.remaining_agents = agent_count;
    app.ui.agent.finished_agents = vec![false; usize::from(agent_count)];
    app.ui.agent.turn_started = Some(web_time::Instant::now());
    app.ui.agent.presence.active = true;
    app.ui.agent.presence.target = selected_nodes.first().map(|node| node.id);
    app.ui.agent.presence.shown = None;
    app.ui.agent.presence.shimmers.clear();
    app.ui.agent.presence.generating = false;
}

impl AgentUi {
    fn new_chat(&mut self) {
        self.draft.clear();
        self.images.clear();
        self.pending_images.clear();
        self.excluded_selection.clear();
        self.turn_started = None;
        self.remaining_agents = 0;
        self.finished_agents.clear();
        self.remaining_variants = 0;
        self.active_variant_count = 0;
        self.active_prompt.clear();
        self.history_open = false;
        self.selected_suggestion = None;
        self.active = None;
        self.busy = false;
        self.permission = None;
    }
}

fn stop_chat(app: &mut LizardApp) {
    app.ui.agent.presence = AgentPresence::default();
    app.services.agent_stop();
    app.services.agent_reset();
    if app.ui.agent.busy {
        let history = app.session.ds().ok().filter(|ds| Some(ds.uid) == app.ui.agent.runtime_document).map(|ds| (ds.history.undo.len(), ds.revision));
        if let Some(chat) = app.ui.agent.runtime_conversation.and_then(|id| app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id)) {
            for message in chat.messages.iter_mut().filter(|message| message.role == "assistant" && message.work_seconds.is_none()) {
                message.stopped = true;
                if let Some((undo_after, revision_after)) = history {
                    message.undo_after = undo_after;
                    message.revision_after = revision_after;
                }
            }
        }
        save_history(app);
    }
    app.ui.agent.busy = false;
    app.ui.agent.remaining_agents = 0;
    app.ui.agent.finished_agents.clear();
    app.ui.agent.remaining_variants = 0;
    app.ui.agent.active_variant_count = 0;
    if app.session.ds().ok().is_some_and(|ds| Some(ds.uid) == app.ui.agent.runtime_document) {
        app.ui.agent.images.append(&mut app.ui.agent.pending_images);
    } else {
        app.ui.agent.pending_images.clear();
    }
    app.ui.agent.turn_started = None;
    app.ui.agent.permission = None;
    app.ui.agent.runtime_document = None;
    app.ui.agent.runtime_conversation = None;
}

fn reset_chat(app: &mut LizardApp) {
    stop_chat(app);
    app.ui.agent.new_chat();
}

fn hit(ui: &mut Ui, id: &'static str, rect: Rect, tip: &'static str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(id), Sense::click()).on_hover_text(tip);
    a11y::button(&response, tip);
    response
}

fn line(ui: &Ui, a: egui::Pos2, b: egui::Pos2, color: Color32) {
    ui.painter().line_segment([a, b], Stroke::new(1.0, color));
}

fn glyph(ui: &Ui, rect: Rect, icon: AgentIcon, size: egui::Vec2, color: Color32) {
    icons::paint_agent(ui.painter(), Rect::from_center_size(rect.center(), size), icon, color);
}

fn thought_chevron(ui: &mut Ui, openness: f32, response: &egui::Response) {
    let (icon, size) = if openness >= 0.5 { (LayerIcon::ChevronDown, vec2(5.24, 3.12)) } else { (LayerIcon::ChevronRight, vec2(3.12, 5.24)) };
    icons::paint_layer(ui.painter(), Rect::from_center_size(response.rect.center(), size), icon, Color32::from_rgb(0xA5, 0xA5, 0xA9));
}

fn work_word(ui: &Ui, id: Id) -> &'static str {
    let now = ui.input(|input| input.time);
    let word_index = ui.ctx().data_mut(|data| {
        let state =
            data.get_temp_mut_or_insert_with(id.with("work_word"), || WorkWordState { index: 0, next_change: now + 2.2, seed: id.value() | 1 });
        if now >= state.next_change {
            state.seed ^= state.seed << 13;
            state.seed ^= state.seed >> 7;
            state.seed ^= state.seed << 17;
            let offset = 1 + state.seed as usize % (WORK_WORDS.len() - 1);
            state.index = (state.index + offset) % WORK_WORDS.len();
            state.next_change = now + 2.0 + ((state.seed >> 16) % 15) as f64 / 10.0;
        }
        state.index
    });
    WORK_WORDS[word_index]
}

fn composer_hover(ui: &Ui, id: &'static str, rect: Rect, response: &egui::Response, dark: bool, reduce_motion: bool) -> f32 {
    let duration = if reduce_motion { 0.0 } else { 0.14 };
    let hover = ui.ctx().animate_bool_with_time(ui.id().with((id, "hover")), response.hovered() || response.has_focus(), duration);
    if hover > 0.0 {
        let alpha = (12.0 * hover).round() as u8;
        let color = if dark { Color32::from_rgba_unmultiplied(255, 255, 255, alpha) } else { Color32::from_rgba_unmultiplied(0, 0, 0, alpha) };
        ui.painter().rect_filled(rect, CornerRadius::same(6), color);
    }
    hover
}

fn message_timestamp(created_at: u64) -> String {
    if created_at == 0 {
        return String::new();
    }
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(created_at, |time| time.as_secs());
    let elapsed = now.saturating_sub(created_at);
    if elapsed < 60 {
        "há instantes".into()
    } else if elapsed < 3_600 {
        let minutes = elapsed / 60;
        format!("há {minutes} {}", if minutes == 1 { "minuto" } else { "minutos" })
    } else if elapsed < 86_400 {
        let hours = elapsed / 3_600;
        format!("há {hours} {}", if hours == 1 { "hora" } else { "horas" })
    } else {
        let Some(local) = i64::try_from(created_at)
            .ok()
            .and_then(|unix| time::OffsetDateTime::from_unix_timestamp(unix).ok())
            .map(|utc| utc.to_offset(time::UtcOffset::local_offset_at(utc).unwrap_or(time::UtcOffset::UTC)))
        else {
            return String::new();
        };
        let weekday = match local.weekday() {
            time::Weekday::Sunday => "DOM",
            time::Weekday::Monday => "SEG",
            time::Weekday::Tuesday => "TER",
            time::Weekday::Wednesday => "QUA",
            time::Weekday::Thursday => "QUI",
            time::Weekday::Friday => "SEX",
            time::Weekday::Saturday => "SÁB",
        };
        let month = match local.month() {
            time::Month::January => "JAN",
            time::Month::February => "FEV",
            time::Month::March => "MAR",
            time::Month::April => "ABR",
            time::Month::May => "MAI",
            time::Month::June => "JUN",
            time::Month::July => "JUL",
            time::Month::August => "AGO",
            time::Month::September => "SET",
            time::Month::October => "OUT",
            time::Month::November => "NOV",
            time::Month::December => "DEZ",
        };
        format!("{weekday} {} de {month} - {:02}:{:02}", local.day(), local.hour(), local.minute())
    }
}

fn receipt_detail(ui: &mut Ui, label: &str, text: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(label).size(11.0).strong().color(Color32::from_rgb(0xA5, 0xA5, 0xA9)));
    ui.add_space(5.0);
    egui::Frame::new().fill(Color32::from_rgb(0x26, 0x26, 0x26)).corner_radius(5).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(text).monospace().size(11.0).color(Color32::from_rgb(0xC9, 0xC9, 0xCD)))
                .wrap_mode(egui::TextWrapMode::Extend),
        );
    });
}

fn receipt(ui: &mut Ui, id: Id, title: &str, tool: &ChatTool, preview: Option<&AgentPreview>, max_width: f32) {
    let mut is_open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(preview.is_some());
    let width = max_width.min(ui.available_width()).max(1.0);
    egui::Frame::new().fill(RECEIPT_BG).stroke(Stroke::new(1.0, RECEIPT_BORDER)).corner_radius(11).show(ui, |ui| {
        ui.set_width(width);
        let header = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
        if header.1.clicked() {
            is_open = !is_open;
            ui.data_mut(|data| data.insert_temp(id, is_open));
        }
        a11y::disclosure(&header.1, "Detalhes da ferramenta", is_open);
        let rect = header.0;
        let max_chars = ((rect.width() - 62.0) / 7.0).floor().max(0.0) as usize;
        let label = if title.chars().count() > max_chars {
            format!("{}…", title.chars().take(max_chars.saturating_sub(1)).collect::<String>())
        } else {
            title.to_string()
        };
        ui.painter().text(pos2(rect.min.x + 11.0, rect.center().y), Align2::LEFT_CENTER, label, regular(13.0), Color32::from_rgb(0xB9, 0xB9, 0xBC));
        let status_color = if tool.status == "completed" { RECEIPT_GREEN } else { Color32::from_rgb(0x94, 0x94, 0x98) };
        ui.painter().text(
            pos2(rect.max.x - 35.0, rect.center().y),
            Align2::CENTER_CENTER,
            if tool.status == "completed" { "✓" } else { "◌" },
            regular(13.0),
            status_color,
        );
        glyph(
            ui,
            Rect::from_center_size(pos2(rect.max.x - 18.0, rect.center().y), vec2(14.0, 14.0)),
            if is_open { AgentIcon::Up } else { AgentIcon::Down },
            vec2(14.0, 14.0),
            Color32::from_rgb(0x85, 0x85, 0x89),
        );
        let animation_time = if ui.style().animation_time > 0.0 { 0.2 } else { 0.0 };
        let openness = ui.ctx().animate_bool_with_time_and_easing(id.with("open"), is_open, animation_time, egui::emath::easing::cubic_in_out);
        if openness > 0.001 {
            let formatted = |value: &serde_json::Value| match value {
                serde_json::Value::String(text) => text.clone(),
                value => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
            };
            let input = tool.input.as_ref().map(formatted);
            let output = tool
                .output
                .as_ref()
                .map(formatted)
                .or_else(|| (!tool.content.is_empty()).then(|| serde_json::to_string_pretty(&tool.content).unwrap_or_default()));
            let detail_height = |text: &str| 10.0 + 14.0 + 5.0 + 16.0 + text.lines().count().min(30) as f32 * 14.0;
            let content_height = (24.0
                + input.as_deref().map_or(0.0, detail_height)
                + output.as_deref().map_or(0.0, detail_height)
                + if input.is_none() && output.is_none() { 28.0 } else { 0.0 }
                + if preview.and_then(|item| item.texture.as_ref()).is_some() { 196.0 } else { 0.0 })
            .min(300.0);
            ui.painter()
                .line_segment([pos2(rect.min.x, rect.max.y), pos2(rect.max.x, rect.max.y)], Stroke::new(1.0, Color32::from_rgb(0x3A, 0x3A, 0x3E)));
            egui::ScrollArea::both()
                .id_salt(id.with("details"))
                .max_width(width)
                .max_height((content_height * openness).max(1.0))
                .min_scrolled_width(0.0)
                .min_scrolled_height(0.0)
                .auto_shrink([false, true])
                .content_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width((width - 24.0).max(1.0));
                    if let Some(input) = &input {
                        receipt_detail(ui, "Parâmetros", input);
                    }
                    if let Some(output) = &output {
                        receipt_detail(ui, "Resposta", output);
                    } else if input.is_none() {
                        ui.label(
                            egui::RichText::new(if tool.status == "in_progress" {
                                "Aguardando resposta da ferramenta…"
                            } else {
                                "Sem detalhes enviados pela ferramenta."
                            })
                            .size(11.0)
                            .color(Color32::from_rgb(0x94, 0x94, 0x98)),
                        );
                    }
                    if let Some(texture) = preview.and_then(|item| item.texture.as_ref()) {
                        ui.add_space(10.0);
                        ui.add(egui::Image::from_texture(texture).max_width((width - 24.0).max(1.0)).max_height(180.0).corner_radius(4));
                    }
                });
        }
    });
}

fn context_chip(ui: &mut Ui, rect: Rect, name: &str, salt: NodeId, t: &Tokens) -> (bool, bool) {
    let body = Rect::from_min_max(rect.min, pos2(rect.max.x - 24.0, rect.max.y));
    let remove = Rect::from_min_max(pos2(body.max.x, rect.min.y), rect.max);
    let locate = ui.interact(body, ui.id().with(("agent_node", &salt)), Sense::click()).on_hover_text("Localizar no canvas");
    let exclude = ui.interact(remove, ui.id().with(("agent_remove_node", &salt)), Sense::click()).on_hover_text("Retirar do contexto do chat");
    a11y::button(&locate, "Localizar no canvas");
    a11y::button(&exclude, "Retirar do contexto do chat");
    ui.painter().rect(
        rect,
        CornerRadius::same(5),
        if locate.hovered() || exclude.hovered() { t.pressed } else { t.field },
        Stroke::new(1.0, t.border),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(pos2(rect.min.x + 9.0, rect.center().y), Align2::LEFT_CENTER, name, regular(11.0), t.text);
    ui.painter().text(remove.center(), Align2::CENTER_CENTER, "×", regular(14.0), t.text_secondary);
    (locate.clicked(), exclude.clicked())
}

pub fn show(app: &mut LizardApp, ui: &mut Ui) {
    if app.ui.agent.connection.as_ref().is_none_or(|(kind, _)| *kind != app.ui.agent.agent) {
        let kind = app.ui.agent.agent;
        app.ui.agent.connection = Some((kind, app.services.agent_connection(kind)));
    }
    if app.ui.agent.models_requested != Some(app.ui.agent.agent) {
        app.ui.agent.models_requested = Some(app.ui.agent.agent);
        if app.ui.agent.agent == AgentKind::Codex {
            app.ui.agent.models_loading_until = Some(web_time::Instant::now() + std::time::Duration::from_secs(5));
            app.services.agent_request_models(app.ui.agent.agent);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let width = full.width();
    let text = t.text;
    let secondary = if t.dark { MUTED } else { t.text_secondary };
    let dim = if t.dark { DIM } else { t.text_tertiary };
    let panel = t.panel;
    let border = t.border;
    let selected_nodes = selected_context(app);
    let context_height = if selected_nodes.is_empty() { 0.0 } else { 34.0 };
    let image_size = 72.0_f32.min((width - 56.0).max(24.0));
    let image_columns = (((width - 56.0 + 8.0) / (image_size + 8.0)).floor() as usize).max(1);
    let image_rows = app.ui.agent.images.len().div_ceil(image_columns);
    let images_height = image_rows as f32 * (image_size + 8.0);
    let composer_height = 142.0 + context_height + images_height;
    ui.painter().rect_filled(full, 0.0, panel);

    // Header: back, two-line title, IA badge, options.
    let header_bottom = full.min.y + 63.0;
    line(ui, pos2(full.min.x, header_bottom - 0.5), pos2(full.max.x, header_bottom - 0.5), border);
    let back = Rect::from_min_size(pos2(full.min.x + 8.0, full.min.y + 19.0), vec2(24.0, 24.0));
    let back_resp = hit(ui, "agent_back", back, "Todos os chats");
    if back_resp.hovered() {
        ui.painter().rect_filled(back, CornerRadius::same(5), t.hover);
    }
    glyph(ui, back, AgentIcon::Back, vec2(5.002, 9.002), text);
    if back_resp.clicked() {
        app.ui.agent.history_open = !app.ui.agent.history_open;
    }
    let title_x = full.min.x + 36.0;
    let title = if app.ui.agent.history_open {
        "Todos os chats"
    } else {
        app.ui
            .agent
            .active
            .and_then(|id| app.ui.agent.conversations.iter().find(|chat| chat.id == id))
            .map_or("Novo chat", |chat| chat.title.as_str())
    };
    ui.painter().text(pos2(title_x, full.min.y + 12.0), Align2::LEFT_TOP, title, semibold(13.0), text);
    ui.painter().text(pos2(title_x, full.min.y + 34.0), Align2::LEFT_TOP, "Compartilhado", regular(11.0), secondary);
    let more = Rect::from_min_size(pos2(full.max.x - 38.0, full.min.y + 12.0), vec2(24.0, 24.0));
    let more_resp = hit(ui, "agent_options", more, "Mais opções");
    if more_resp.hovered() || a11y::menu_open(&more_resp) {
        ui.painter().rect_filled(more, CornerRadius::same(5), t.hover);
    }
    glyph(ui, more, AgentIcon::More, vec2(15.0, 3.0), text);
    let badge = Rect::from_center_size(pos2(full.max.x - 55.5, full.min.y + 24.0), vec2(19.0, 18.0));
    ui.painter().rect_stroke(badge, CornerRadius::same(5), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter().text(badge.center(), Align2::CENTER_CENTER, "IA", semibold(11.0), secondary);
    if hit(ui, "agent_model_badge", badge, "Selecionar modelo").clicked() {
        egui::Popup::open_id(ui.ctx(), Id::new("agent_footer_model_popup"));
    }

    let middle_bottom = (full.max.y - 26.0 - composer_height).max(header_bottom);
    let middle = Rect::from_min_max(pos2(full.min.x, header_bottom), pos2(full.max.x, middle_bottom));
    let painter = ui.painter().with_clip_rect(middle);
    if app.ui.agent.history_open {
        let new_chat = Rect::from_min_size(pos2(full.min.x + 16.0, header_bottom + 16.0), vec2((width - 32.0).max(0.0), 42.0));
        let resp = hit(ui, "agent_new_chat", new_chat, "Novo chat");
        painter.rect(new_chat, CornerRadius::same(13), panel, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        glyph(ui, Rect::from_center_size(pos2(new_chat.min.x + 21.0, new_chat.center().y), vec2(24.0, 24.0)), AgentIcon::Add, vec2(12.0, 12.0), text);
        painter.text(pos2(new_chat.min.x + 40.0, new_chat.center().y), Align2::LEFT_CENTER, "Novo chat", regular(13.0), text);
        let key = document_key(app);
        let chats: Vec<_> = app.ui.agent.conversations.iter().filter(|chat| chat.document == key).cloned().collect();
        let mut chosen = None;
        let list = Rect::from_min_max(pos2(full.min.x + 8.0, new_chat.max.y + 8.0), middle.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            if chats.is_empty() {
                ui.label(egui::RichText::new("Nenhum chat anterior").size(11.0).color(secondary));
            }
            for chat in chats.iter().rev() {
                if ui.add_sized([list.width() - 16.0, 36.0], egui::Button::new(egui::RichText::new(&chat.title).size(12.0)).frame(false)).clicked() {
                    chosen = Some((chat.id, chat.agent));
                }
            }
        });
        if let Some((id, agent)) = chosen {
            stop_chat(app);
            app.ui.agent.active = Some(id);
            app.ui.agent.agent = agent;
            app.ui.agent.busy = false;
            app.ui.agent.runtime_document = None;
            app.ui.agent.runtime_conversation = None;
            app.ui.agent.history_open = false;
        }
        if resp.clicked() {
            reset_chat(app);
        }
    } else if let Some(chat) = app.ui.agent.active.and_then(|id| app.ui.agent.conversations.iter().find(|chat| chat.id == id)).cloned() {
        let transcript = middle.shrink2(vec2(20.0, 12.0));
        let content_width = (transcript.width() - TRANSCRIPT_SCROLL_GUTTER).max(1.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(transcript));
        let mut revert = None;
        let mut locate_node = None;
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.set_width(content_width);
            for (index, message) in chat.messages.iter().enumerate() {
                if message.role == "user" {
                    ui.add_space(12.0);
                    let bubble_width = content_width.min(320.0);
                    ui.horizontal(|ui| {
                        ui.add_space((content_width - bubble_width).max(0.0));
                        ui.vertical(|ui| {
                            ui.set_width(bubble_width);
                            ui.allocate_ui_with_layout(vec2(bubble_width, 18.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(message_timestamp(message.created_at)).size(12.0).color(Color32::from_rgb(0x92, 0x92, 0x98)),
                                );
                                ui.label(egui::RichText::new("Você").size(12.0).strong().color(Color32::from_rgb(0xC9, 0xC9, 0xCD)));
                            });
                            ui.add_space(5.0);
                            egui::Frame::new()
                                .fill(CHAT_BUBBLE)
                                .corner_radius(CornerRadius { nw: 24, ne: 0, sw: 24, se: 24 })
                                .inner_margin(egui::Margin::symmetric(16, 12))
                                .show(ui, |ui| {
                                    ui.set_width((bubble_width - 32.0).max(0.0));
                                    for node in &message.selected_nodes {
                                        let badge = egui::Button::new(
                                            egui::RichText::new(format!("⌗ {}", node.name)).size(12.0).color(Color32::from_rgb(0xA8, 0xD9, 0xFC)),
                                        )
                                        .fill(BLUE_BG)
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(0x52, 0x65, 0x89)))
                                        .corner_radius(5);
                                        if ui.add(badge).on_hover_text("Localizar no canvas").clicked() {
                                            locate_node = Some(node.id);
                                        }
                                    }
                                    if !message.text.is_empty() {
                                        ui.label(egui::RichText::new(&message.text).size(14.0).color(Color32::from_rgb(0xD1, 0xD1, 0xD4)));
                                    }
                                    for image in &message.attached_images {
                                        ui.label(egui::RichText::new(format!("▧ {image}")).size(11.0).color(Color32::from_rgb(0xA8, 0xD9, 0xFC)));
                                    }
                                });
                        });
                    });
                } else {
                    ui.add_space(9.0);
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(9.0, 9.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.5, Color32::from_rgb(0xF1, 0x6A, 0x99));
                        ui.add_space(1.0);
                        let agent_name =
                            if message.parallel { format!("Lizard Agent {}", message.agent_index + 1) } else { "Lizard Agent".to_string() };
                        ui.label(egui::RichText::new(agent_name).size(12.0).color(Color32::from_rgb(0xD1, 0xD1, 0xD4)));
                        if !message.model_label.is_empty() {
                            ui.label(egui::RichText::new(&message.model_label).size(12.0).color(Color32::from_rgb(0x85, 0x85, 0x8A)));
                        }
                        if message.variant_total > 1 {
                            ui.label(
                                egui::RichText::new(format!("Variação {}", message.variant_index))
                                    .size(12.0)
                                    .color(Color32::from_rgb(0x85, 0x85, 0x8A)),
                            );
                        }
                    });
                    let working = app.ui.agent.busy
                        && app.ui.agent.runtime_conversation == Some(chat.id)
                        && message.work_seconds.is_none()
                        && !chat.messages.iter().skip(index + 1).any(|later| later.role == "assistant" && later.agent_index == message.agent_index);
                    if working || !message.tools.is_empty() || !message.thinking.is_empty() {
                        ui.horizontal(|ui| {
                            if working {
                                ui.vertical(|ui| {
                                    let header_id = ui.make_persistent_id(("agent_work", chat.id, index));
                                    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), header_id, false);
                                    let header = ui.horizontal(|ui| {
                                        let spacing = ui.spacing().item_spacing.x;
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        state.show_toggle_button(ui, thought_chevron);
                                        ui.spacing_mut().item_spacing.x = spacing;
                                        let word = work_word(ui, header_id);
                                        let shimmer = ShimmerText::new(word)
                                            .direction(ShimmerDirection::Diagonal)
                                            .duration(0.6)
                                            .size(12.0)
                                            .colors(Color32::from_rgb(0x94, 0x94, 0x98), Color32::from_rgb(0xD1, 0xD1, 0xD4))
                                            .show(ui);
                                        let click = ui.interact(shimmer.rect, header_id.with("label"), Sense::click());
                                        if click.clicked() {
                                            state.toggle(ui);
                                        }
                                        a11y::disclosure(&click, word, state.is_open());
                                    });
                                    state.show_body_indented(&header.response, ui, |ui| {
                                        if !message.thinking.is_empty() {
                                            ui.label(egui::RichText::new(&message.thinking).size(12.0).color(secondary));
                                        }
                                    });
                                });
                            } else {
                                let label = message.work_seconds.map_or_else(|| "Pensando…".to_string(), |s| format!("Thought for {s}s"));
                                egui::CollapsingHeader::new(egui::RichText::new(label).size(12.0).color(Color32::from_rgb(0x94, 0x94, 0x98)))
                                    .id_salt(("agent_work", chat.id, index))
                                    .default_open(false)
                                    .icon(thought_chevron)
                                    .show(ui, |ui| {
                                        if !message.thinking.is_empty() {
                                            ui.label(egui::RichText::new(&message.thinking).size(12.0).color(secondary));
                                        }
                                    });
                            }
                        });
                    }
                    for (tool_index, tool) in message.tools.iter().enumerate() {
                        ui.add_space(9.0);
                        let preview = if tool_index + 1 == message.tools.len() { message.previews.first() } else { None };
                        let title =
                            if tool.title.contains("screenshot") || tool.title.contains("execute") { "Designed" } else { tool.title.as_str() };
                        receipt(ui, ui.id().with(("agent_receipt", chat.id, index, tool_index)), title, tool, preview, content_width);
                    }
                    if message.tools.is_empty() && !message.previews.is_empty() {
                        ui.add_space(9.0);
                        let preview_tool = ChatTool { status: "completed".into(), ..Default::default() };
                        receipt(
                            ui,
                            ui.id().with(("agent_receipt", chat.id, index, 0)),
                            "Designed",
                            &preview_tool,
                            message.previews.first(),
                            content_width,
                        );
                    }
                    if !message.text.is_empty() {
                        ui.add_space(9.0);
                        crate::agent_markdown::show(ui, &message.text, Color32::from_rgb(0xB9, 0xB9, 0xBC), BLUE);
                    }
                    if message.stopped {
                        ui.label(egui::RichText::new("Interrompido").size(11.0).color(secondary));
                    }
                    if let Some(seconds) = message.work_seconds {
                        ui.add_space(9.0);
                        ui.label(egui::RichText::new(format!("Designed for {seconds}s")).size(14.0).color(Color32::from_rgb(0x94, 0x94, 0x98)));
                    }
                    if !app.ui.agent.busy
                        && !message.parallel
                        && !message.reverted
                        && message.undo_after > message.undo_before
                        && app.session.ds().ok().is_some_and(|ds| {
                            ds.history.undo.len() == message.undo_after
                                && ds.revision == message.revision_after
                                && ds
                                    .history
                                    .undo
                                    .get(message.undo_before..message.undo_after)
                                    .is_some_and(|entries| entries.iter().all(|entry| entry.agent))
                        })
                        && ui.small_button("Desfazer alterações desta resposta").clicked()
                    {
                        revert = Some((index, message.undo_after - message.undo_before));
                    }
                }
            }
        });
        if let Some(node) = locate_node {
            app.run("select.set", serde_json::json!({"ids":[node]}));
            app.run("view.zoomSelection", serde_json::json!({}));
        }
        if let Some((index, count)) = revert {
            for _ in 0..count.min(100) {
                let _ = app.run("edit.undo", serde_json::json!({}));
            }
            if let Some(message) = app.ui.agent.conversations.iter_mut().find(|c| c.id == chat.id).and_then(|c| c.messages.get_mut(index)) {
                message.reverted = true;
            }
            save_history(app);
        }
    } else {
        // The reference's 980 px column has fixed 63 + 705 + 156 px sections and distributes
        // the remaining 56 px as two 28 px gaps. Smaller windows collapse those gaps first.
        let section_gap = ((full.height() - 924.0) / 2.0).max(0.0);
        let welcome_top = header_bottom + section_gap + 16.0;
        let emblem = Rect::from_center_size(pos2(full.center().x, welcome_top + 36.0), vec2(40.0, 40.0));
        painter.circle_filled(emblem.center(), 20.0, BLUE_BG);
        glyph(ui, emblem, AgentIcon::Emblem, vec2(16.5, 16.5), BLUE);
        painter.text(pos2(full.center().x, welcome_top + 68.0), Align2::CENTER_TOP, "E depois?", semibold(15.0), text);
        let labels = ["Organizar e polir", "Gerar designs", "Criar plugins e shaders"];
        let prompts =
            ["Organize e refine este design.", "Crie uma nova proposta visual para esta tela.", "Crie um plugin ou shader para este documento."];
        let cards_top = welcome_top + 113.0;
        for (index, label) in labels.iter().enumerate() {
            let card = Rect::from_min_size(pos2(full.min.x + 16.0, cards_top + index as f32 * 50.0), vec2((width - 32.0).max(0.0), 42.0));
            let resp = hit(
                ui,
                match index {
                    0 => "agent_suggest_polish",
                    1 => "agent_suggest_generate",
                    _ => "agent_suggest_plugins",
                },
                card,
                label,
            );
            let fill = if resp.hovered() || app.ui.agent.selected_suggestion == Some(index) { t.hover } else { panel };
            painter.rect(card, CornerRadius::same(13), fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
            let ir = Rect::from_center_size(pos2(card.min.x + 21.0, card.center().y), vec2(24.0, 24.0));
            let (kind, size) = match index {
                0 => (AgentIcon::Polish, vec2(13.499, 12.999)),
                1 => (AgentIcon::Generate, vec2(14.0, 14.0)),
                _ => (AgentIcon::Plugins, vec2(14.0, 13.0)),
            };
            glyph(ui, ir, kind, size, secondary);
            painter.text(pos2(card.min.x + 40.0, card.center().y), Align2::LEFT_CENTER, *label, regular(13.0), text);
            let chevron_box = Rect::from_center_size(pos2(card.max.x - 22.0, card.center().y), vec2(28.0, 24.0));
            glyph(ui, chevron_box, AgentIcon::Chevron, vec2(9.002, 5.002), dim);
            if resp.clicked() {
                app.ui.agent.selected_suggestion = Some(index);
                app.ui.agent.draft = prompts[index].to_string();
                ui.memory_mut(|m| m.request_focus(Id::new("agent_prompt")));
            }
        }
        let connection = app.ui.agent.connection.as_ref().map(|(_, state)| state.clone()).unwrap_or_default();
        let setup = Rect::from_min_size(pos2(full.min.x + 16.0, cards_top + 160.0), vec2((width - 32.0).max(0.0), 54.0));
        painter.rect(setup, CornerRadius::same(8), panel, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        let setup_title = if app.ui.agent.login_pending {
            "Aguardando login no navegador…"
        } else if !connection.installed {
            "Instalar agente"
        } else if !connection.authenticated {
            "Conectar com ChatGPT"
        } else {
            if app.ui.agent.agent == AgentKind::Codex { "Agente conectado" } else { "Agente disponível" }
        };
        painter.text(pos2(setup.min.x + 12.0, setup.min.y + 9.0), Align2::LEFT_TOP, setup_title, semibold(12.0), text);
        painter.text(pos2(setup.min.x + 12.0, setup.min.y + 29.0), Align2::LEFT_TOP, &connection.detail, regular(11.0), secondary);
        if hit(ui, "agent_setup", setup, setup_title).clicked() && !app.ui.agent.login_pending {
            if !connection.installed {
                let url = match app.ui.agent.agent {
                    AgentKind::Codex => "https://developers.openai.com/codex/cli",
                    AgentKind::Claude => "https://github.com/zed-industries/claude-agent-acp",
                    AgentKind::Gemini => "https://github.com/google-gemini/gemini-cli",
                    AgentKind::Kimi => "https://www.kimi.com/code/docs/en/kimi-code-cli/guides/ides",
                };
                app.services.open_url(url);
            } else if !connection.authenticated && app.ui.agent.agent == AgentKind::Codex {
                match app.services.agent_login() {
                    Ok(()) => app.ui.agent.login_pending = true,
                    Err(error) => app.toast(error),
                }
            }
        }
    }

    // Composer is anchored to the bottom, independently of the welcome area height.
    let composer = Rect::from_min_size(pos2(full.min.x + 16.0, full.max.y - 16.0 - composer_height), vec2((width - 32.0).max(0.0), composer_height));
    ui.painter().rect(composer, CornerRadius::same(13), panel, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    if !app.ui.agent.images.is_empty() {
        let mut remove_image = None;
        for (index, image) in app.ui.agent.images.iter_mut().enumerate() {
            if image.preview.is_none()
                && let Ok(decoded) = image::load_from_memory(&image.bytes)
            {
                let thumb = decoded.resize_to_fill(144, 144, image::imageops::FilterType::Triangle).to_rgba8();
                let color = egui::ColorImage::from_rgba_unmultiplied([thumb.width() as usize, thumb.height() as usize], thumb.as_raw());
                image.preview = Some(ui.ctx().load_texture(format!("agent-image-{}-{index}", image.name), color, egui::TextureOptions::LINEAR));
            }
            let row = index / image_columns;
            let column = index % image_columns;
            let tile = Rect::from_min_size(
                pos2(composer.min.x + 12.0 + column as f32 * (image_size + 8.0), composer.min.y + 8.0 + row as f32 * (image_size + 8.0)),
                vec2(image_size, image_size),
            );
            let _ = ui.interact(tile, ui.id().with(("agent_image", index)), Sense::hover()).on_hover_text(&image.name);
            ui.painter().rect(tile, CornerRadius::same(8), t.field, Stroke::new(1.0, border), egui::StrokeKind::Inside);
            if let Some(preview) = &image.preview {
                egui::Image::from_texture(preview).corner_radius(8).paint_at(ui, tile.shrink(1.0));
            }
            let remove = Rect::from_min_size(pos2(tile.max.x - 24.0, tile.min.y + 4.0), vec2(20.0, 20.0));
            let remove_resp = ui.interact(remove, ui.id().with(("agent_remove_image", index)), Sense::click()).on_hover_text("Remover imagem");
            a11y::button(&remove_resp, "Remover imagem");
            ui.painter().circle_filled(remove.center(), 10.0, if remove_resp.hovered() { t.pressed } else { panel });
            ui.painter().circle_stroke(remove.center(), 9.5, Stroke::new(1.0, border));
            ui.painter().text(remove.center(), Align2::CENTER_CENTER, "×", regular(14.0), text);
            if remove_resp.clicked() {
                remove_image = Some(index);
            }
        }
        if let Some(index) = remove_image {
            app.ui.agent.images.remove(index);
        }
    }
    if !selected_nodes.is_empty() {
        let mut x = composer.min.x + 12.0;
        let mut shown = 0;
        for node in &selected_nodes {
            let label: String = node.name.chars().take(22).collect();
            let chip_width = (label.chars().count() as f32 * 6.2 + 42.0).clamp(72.0, 190.0);
            if x + chip_width > composer.max.x - 12.0 {
                break;
            }
            let chip = Rect::from_min_size(pos2(x, composer.min.y + 8.0 + images_height), vec2(chip_width, 24.0));
            let (locate, exclude) = context_chip(ui, chip, &label, node.id, &t);
            if locate {
                app.run("select.set", serde_json::json!({"ids":[node.id]}));
                app.run("view.zoomSelection", serde_json::json!({}));
            }
            if exclude && !app.ui.agent.excluded_selection.contains(&node.id) {
                app.ui.agent.excluded_selection.push(node.id);
            }
            x += chip_width + 6.0;
            shown += 1;
        }
        if shown < selected_nodes.len() {
            ui.painter().text(
                pos2(x + 3.0, composer.min.y + 20.0 + images_height),
                Align2::LEFT_CENTER,
                format!("+{}", selected_nodes.len() - shown),
                regular(11.0),
                secondary,
            );
        }
    }
    let input = Rect::from_min_max(
        pos2(composer.min.x + 16.0, composer.min.y + 16.0 + context_height + images_height),
        pos2(composer.max.x - 16.0, composer.min.y + 98.0 + context_height + images_height),
    );
    paste_image_into_composer(app, ui);
    let mut input_ui = ui.new_child(egui::UiBuilder::new().max_rect(input));
    let input_resp = egui::ScrollArea::vertical()
        .id_salt("agent_prompt_scroll")
        .auto_shrink([false, false])
        .show(&mut input_ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut app.ui.agent.draft)
                    .id(Id::new("agent_prompt"))
                    .font(regular(13.0))
                    .hint_text(egui::RichText::new("Solicitar alterações").size(13.0).color(secondary))
                    .frame(egui::Frame::NONE)
                    .desired_width(ui.available_width())
                    .desired_rows(3),
            )
        })
        .inner;
    let actions_y = composer.max.y - 32.0;
    let attach = Rect::from_min_size(pos2(composer.min.x + 8.0, actions_y), vec2(24.0, 24.0));
    let parallel = Rect::from_min_size(pos2(attach.max.x + 8.0, actions_y), vec2(34.0, 24.0));
    let variants = Rect::from_min_size(pos2(parallel.max.x + 11.0, actions_y), vec2(34.0, 24.0));
    let send = Rect::from_min_size(pos2(composer.max.x - 32.0, actions_y), vec2(24.0, 24.0));
    let model_label = app
        .ui
        .agent
        .model
        .as_deref()
        .and_then(|id| app.ui.agent.models.iter().find(|(model_id, _)| model_id == id).map(|(_, label)| label.as_str()))
        .unwrap_or_else(|| app.ui.agent.agent.label());
    let model_x = variants.max.x + 11.0;
    let model_available = (send.min.x - 8.0 - model_x).max(0.0);
    let model_label_width = ui.painter().layout_no_wrap(model_label.to_owned(), regular(12.0), Color32::PLACEHOLDER).size().x;
    let model_width = (model_label_width + 36.0).ceil().max(72.0).min(model_available);
    let model = Rect::from_min_size(pos2(model_x, actions_y), vec2(model_width, 24.0));
    let attach_resp = hit(ui, "agent_context", attach, "Inserir anexo");
    let parallel_resp = hit(ui, "agent_parallel", parallel, "Quantidade de agentes");
    let variants_resp = hit(ui, "agent_variants", variants, "Quantidade de variantes");
    let model_resp = hit(ui, "agent_footer_model", model, "Selecionar modelo");
    ui.painter().rect_filled(model, CornerRadius::same(5), t.field);
    a11y::combo(&model_resp, "Selecionar modelo", model_label, egui::Popup::is_id_open(ui.ctx(), Id::new("agent_footer_model_popup")));
    let send_resp = hit(ui, "agent_send", send, if app.ui.agent.busy { "Parar agente" } else { "Enviar mensagem" });
    let reduce_motion = app.reduce_motion();
    for (id, rect, response) in
        [("agent_context", attach, &attach_resp), ("agent_parallel", parallel, &parallel_resp), ("agent_variants", variants, &variants_resp)]
    {
        composer_hover(ui, id, rect, response, t.dark, reduce_motion);
    }
    let model_hover = ui.ctx().animate_bool_with_time(
        ui.id().with("agent_footer_model_hover"),
        model_resp.hovered() || model_resp.has_focus(),
        if reduce_motion { 0.0 } else { 0.14 },
    );
    if model_hover > 0.0 {
        ui.painter().rect_stroke(
            model,
            CornerRadius::same(5),
            Stroke::new(1.0, t.field_hover_border.linear_multiply(model_hover)),
            egui::StrokeKind::Inside,
        );
    }
    glyph(ui, attach, AgentIcon::Paperclip, vec2(14.0, 14.0), Color32::from_rgb(0xA5, 0xA5, 0xA8));
    let parallel_color = Color32::from_rgb(0xFF, 0xBF, 0x00);
    glyph(ui, Rect::from_min_size(parallel.min, vec2(13.0, 24.0)), AgentIcon::Lightning, vec2(13.0, 13.0), parallel_color);
    ui.painter().text(
        pos2(parallel.min.x + 16.0, parallel.center().y),
        Align2::LEFT_CENTER,
        format!("{}x", app.ui.agent.agent_count.max(1)),
        regular(12.0),
        parallel_color,
    );
    glyph(ui, Rect::from_min_size(variants.min, vec2(13.0, 24.0)), AgentIcon::Grid, vec2(13.0, 13.0), Color32::from_rgb(0x99, 0x99, 0x99));
    ui.painter().text(
        pos2(variants.min.x + 16.0, variants.center().y),
        Align2::LEFT_CENTER,
        format!("{}x", app.ui.agent.variant_count.max(1)),
        regular(12.0),
        Color32::from_rgb(0x99, 0x99, 0x99),
    );
    let model_name = crate::widgets::elide_font(ui, model_label, (model.width() - 36.0).max(0.0), regular(12.0));
    if model.width() >= 36.0 {
        ui.painter().text(
            pos2(model.min.x + 8.0, model.center().y),
            Align2::LEFT_CENTER,
            model_name,
            regular(12.0),
            Color32::from_rgb(0xAA, 0xAA, 0xAA),
        );
    }
    if model.width() >= 24.0 {
        glyph(
            ui,
            Rect::from_center_size(pos2(model.max.x - 12.0, model.center().y), vec2(11.0, 11.0)),
            AgentIcon::Down,
            vec2(11.0, 11.0),
            Color32::from_rgb(0x77, 0x77, 0x77),
        );
    }
    let send_hover = ui.ctx().animate_bool_with_time(
        ui.id().with("agent_send_hover"),
        send_resp.hovered() || send_resp.has_focus(),
        if reduce_motion { 0.0 } else { 0.14 },
    );
    let send_blue = Color32::from_rgb(
        (12.0 + 18.0 * send_hover).round() as u8,
        (140.0 + 13.0 * send_hover).round() as u8,
        (232.0 + 7.0 * send_hover).round() as u8,
    );
    ui.painter().circle_filled(send.center(), 12.0, send_blue);
    if app.ui.agent.busy {
        ui.painter().rect_filled(Rect::from_center_size(send.center(), vec2(7.0, 7.0)), 1.0, Color32::from_rgb(0xDD, 0xE1, 0xED));
    } else {
        glyph(ui, send, AgentIcon::SendUp, vec2(15.0, 15.0), Color32::from_rgb(0xDD, 0xE1, 0xED));
    }
    let enter = input_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if send_resp.clicked() || enter {
        if app.ui.agent.busy && send_resp.clicked() {
            stop_chat(app);
        } else if !app.ui.agent.busy {
            send_prompt(app);
        }
    }

    Menu::popup(&attach_resp).width(176.0).align(egui::RectAlign::TOP_START).show(|ui| {
        if MenuItem::new("Anexar PNG ou JPEG").show(ui).clicked() {
            app.pick_open("Anexar imagem ao agente", &["png", "jpg", "jpeg"], |app, path| {
                let result = std::fs::metadata(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|meta| {
                        if meta.len() <= MAX_IMAGE_BYTES as u64 {
                            std::fs::read(&path).map_err(|error| error.to_string())
                        } else {
                            Err("A imagem excede 4 MB.".into())
                        }
                    })
                    .and_then(|bytes| {
                        attach_image(app, path.file_name().map_or_else(|| "Imagem".into(), |name| name.to_string_lossy().into_owned()), bytes)
                    });
                if let Err(error) = result {
                    app.toast(error);
                }
            });
            ui.close();
        }
        if MenuItem::new("Colar imagem").show(ui).clicked() {
            attach_clipboard_image(app);
            ui.close();
        }
    });

    if let Some(AgentPermission { id, title, options }) = app.ui.agent.permission.clone() {
        egui::Window::new("Permissão do agente").collapsible(false).resizable(false).show(ui.ctx(), |ui| {
            ui.label(title);
            ui.horizontal(|ui| {
                for (option, label) in options {
                    if ui.button(label).clicked() {
                        app.services.agent_permission(id.clone(), Some(option));
                        app.ui.agent.permission = None;
                    }
                }
                if ui.button("Recusar").clicked() {
                    app.services.agent_permission(id, None);
                    app.ui.agent.permission = None;
                }
            });
        });
    }

    Menu::popup(&more_resp).width(166.0).show(|ui| {
        if MenuItem::new("Novo chat").show(ui).clicked() {
            reset_chat(app);
            ui.close();
        }
        Menu::separator(ui, None);
        for kind in [AgentKind::Codex, AgentKind::Claude, AgentKind::Gemini, AgentKind::Kimi] {
            if MenuItem::new(kind.label()).checked(app.ui.agent.agent == kind).show(ui).clicked() {
                reset_chat(app);
                app.ui.agent.agent = kind;
                app.ui.agent.connection = None;
                app.ui.agent.model = None;
                app.ui.agent.models.clear();
                app.ui.agent.models_requested = None;
                app.ui.agent.models_loading_until = None;
                ui.close();
            }
        }
        Menu::separator(ui, None);
        let setup_label = if app.ui.agent.agent == AgentKind::Codex { "Conexão ChatGPT" } else { "Configurar agente" };
        if MenuItem::new(setup_label).show(ui).clicked() {
            let connected = app.ui.agent.connection.as_ref().is_some_and(|(_, state)| state.authenticated);
            app.ui.agent.connection = None;
            if app.ui.agent.agent == AgentKind::Codex && !connected && !app.ui.agent.login_pending {
                match app.services.agent_login() {
                    Ok(()) => app.ui.agent.login_pending = true,
                    Err(error) => app.toast(error),
                }
            } else if app.ui.agent.agent != AgentKind::Codex {
                let url = match app.ui.agent.agent {
                    AgentKind::Codex => "https://developers.openai.com/codex/cli",
                    AgentKind::Claude => "https://github.com/zed-industries/claude-agent-acp",
                    AgentKind::Gemini => "https://github.com/google-gemini/gemini-cli",
                    AgentKind::Kimi => "https://www.kimi.com/code/docs/en/kimi-code-cli/guides/ides",
                };
                app.services.open_url(url);
            }
            ui.close();
        }
    });
    let models = app.ui.agent.models.clone();
    crate::right::menu_popup(&model_resp).id(Id::new("agent_footer_model_popup")).width(model.width().max(180.0)).show(|ui| {
        if crate::right::menu_item(ui, "Modelo padrão", "") {
            app.ui.agent.model = None;
            ui.close();
        }
        egui::ScrollArea::vertical().max_height(8.0 * 26.0).show(ui, |ui| {
            for (id, label) in &models {
                if crate::right::menu_item(ui, label, "") {
                    app.ui.agent.model = Some(id.clone());
                    ui.close();
                }
            }
        });
    });
    Menu::popup(&parallel_resp).width(196.0).align(egui::RectAlign::TOP_START).show(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Agentes em paralelo").size(11.0).color(ui.visuals().text_color()));
        ui.add_space(4.0);
        for count in 1..=6_u8 {
            let label = format!("{count}x");
            if MenuItem::new(&label).checked(app.ui.agent.agent_count.max(1) == count).show(ui).clicked() {
                app.ui.agent.agent_count = count;
            }
        }
        Menu::separator(ui, None);
        for (label, mode) in [("Dividir trabalho", ParallelMode::Split), ("Lado a lado", ParallelMode::SideBySide)] {
            if MenuItem::new(label).checked(app.ui.agent.parallel_mode == mode).show(ui).clicked() {
                app.ui.agent.parallel_mode = mode;
            }
        }
    });
    Menu::popup(&variants_resp).width(192.0).align(egui::RectAlign::TOP_START).show(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Variantes").size(11.0).color(ui.visuals().text_color()));
        ui.add_space(4.0);
        for count in 1..=6_u8 {
            let label = if count == 1 { "1x · Sem iteração".to_owned() } else { format!("{count}x") };
            if MenuItem::new(&label).checked(app.ui.agent.variant_count.max(1) == count).show(ui).clicked() {
                app.ui.agent.variant_count = count;
            }
        }
        Menu::separator(ui, None);
        for (label, mode) in [("Variar layout", VariantMode::Layout), ("Variar estilo", VariantMode::Style)] {
            if MenuItem::new(label).checked(app.ui.agent.variant_mode == mode).show(ui).clicked() {
                app.ui.agent.variant_mode = mode;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoServices, Services};
    use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
    use lizard_engine::Session;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct FakeAgent {
        sent: Arc<Mutex<Vec<AgentPrompt>>>,
        events: Arc<Mutex<Vec<AgentEvent>>>,
    }

    impl Services for FakeAgent {
        fn agent_send(&mut self, _agent: AgentKind, prompt: AgentPrompt, _document: u64) -> Result<(), String> {
            self.sent.lock().unwrap().push(prompt);
            Ok(())
        }

        fn agent_poll(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
    }

    struct ImagePasteClipboard;

    impl Services for ImagePasteClipboard {
        fn clipboard_get_text(&mut self) -> Option<String> {
            Some("image alt text".into())
        }

        fn clipboard_get_image(&mut self) -> Option<(u32, u32, Vec<u8>)> {
            Some((2, 2, vec![255; 16]))
        }
    }

    fn paste_into_agent(events: Vec<Event>) {
        let mut app = LizardApp::new(Session::new(), Box::new(ImagePasteClipboard));
        app.ui.left_tab = crate::LeftTab::Agent;
        let ctx = egui::Context::default();
        for (time, events) in [(0.0, Vec::new()), (1.0 / 60.0, Vec::new()), (2.0 / 60.0, events)] {
            if time > 1.0 / 60.0 {
                ctx.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
            }
            let input =
                RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))), time: Some(time), events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| app.ui(ui));
            output.textures_delta.clear();
        }
        assert_eq!(app.ui.agent.images.len(), 1, "the clipboard image becomes a composer attachment");
        assert!(app.ui.agent.draft.is_empty(), "the image's alt text must not enter the prompt");
    }

    #[test]
    fn paste_event_with_alt_text_attaches_an_image_to_agent_composer() {
        paste_into_agent(vec![Event::Paste("image alt text".into())]);
    }

    #[test]
    fn cmd_v_key_and_paste_event_attach_only_once_to_agent_composer() {
        paste_into_agent(vec![
            Event::Key { key: Key::V, physical_key: Some(Key::V), pressed: true, repeat: false, modifiers: Modifiers::MAC_CMD | Modifiers::COMMAND },
            Event::Paste("image alt text".into()),
        ]);
    }

    #[test]
    fn sending_a_prompt_streams_an_answer_into_local_chat() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events: events.clone() }));
        app.ui.agent.history_loaded = true;
        app.ui.agent.draft = "Crie um frame".into();
        send_prompt(&mut app);
        assert!(app.ui.agent.busy);
        assert!(app.ui.agent.draft.is_empty());
        assert!(sent.lock().unwrap()[0].text.contains("Crie um frame"));
        assert!(sent.lock().unwrap()[0].text.contains("Lizard agent instructions:"));
        events.lock().unwrap().extend([
            AgentEvent::Session("remote-session".into()),
            AgentEvent::Text("Frame criado.".into()),
            AgentEvent::Done("end_turn".into()),
        ]);
        sync(&mut app, &egui::Context::default());
        assert!(!app.ui.agent.busy);
        assert_eq!(app.ui.agent.conversations[0].messages[1].text, "Frame criado.");
        app.ui.agent.draft = "Mude a cor".into();
        send_prompt(&mut app);
        let sent = sent.lock().unwrap();
        assert_eq!(sent[1].resume.as_deref(), Some("remote-session"));
        assert!(!sent[1].text.contains("Lizard agent instructions:"));
        assert!(sent[1].text.contains("Current document snapshot"));
    }

    #[test]
    fn parallel_agents_complete_independently_before_the_next_variant() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events: events.clone() }));
        app.ui.agent.history_loaded = true;
        app.ui.agent.agent_count = 2;
        app.ui.agent.variant_count = 2;
        app.ui.agent.parallel_mode = ParallelMode::SideBySide;
        app.ui.agent.variant_mode = VariantMode::Layout;
        app.ui.agent.draft = "Desenhe duas opções".into();
        send_prompt(&mut app);
        assert_eq!(sent.lock().unwrap()[0].agent_count, 2);
        assert_eq!(sent.lock().unwrap()[0].variant_count, 2);
        events.lock().unwrap().extend([
            AgentEvent::Scoped { index: 0, event: Box::new(AgentEvent::Session("session-a".into())) },
            AgentEvent::Scoped { index: 1, event: Box::new(AgentEvent::Session("session-b".into())) },
            AgentEvent::Scoped { index: 1, event: Box::new(AgentEvent::Text("Opção B".into())) },
            AgentEvent::Scoped { index: 0, event: Box::new(AgentEvent::Text("Opção A".into())) },
            AgentEvent::Scoped { index: 0, event: Box::new(AgentEvent::Done("end_turn".into())) },
            AgentEvent::Scoped { index: 0, event: Box::new(AgentEvent::Done("end_turn".into())) },
        ]);
        sync(&mut app, &egui::Context::default());
        assert!(app.ui.agent.busy);
        assert_eq!(app.ui.agent.remaining_agents, 1);
        assert_eq!(sent.lock().unwrap().len(), 1);
        assert_eq!(app.ui.agent.conversations[0].messages[1].text, "Opção A");
        assert_eq!(app.ui.agent.conversations[0].messages[2].text, "Opção B");
        assert_eq!(app.ui.agent.conversations[0].remote_sessions, [Some("session-a".into()), Some("session-b".into())]);
        events.lock().unwrap().push(AgentEvent::Scoped { index: 1, event: Box::new(AgentEvent::Done("end_turn".into())) });
        sync(&mut app, &egui::Context::default());
        assert!(app.ui.agent.busy);
        assert_eq!(sent.lock().unwrap().len(), 2);
        assert_eq!(app.ui.agent.conversations[0].messages.len(), 5);
        events.lock().unwrap().extend([
            AgentEvent::Scoped { index: 0, event: Box::new(AgentEvent::Session("variant-a".into())) },
            AgentEvent::Scoped { index: 1, event: Box::new(AgentEvent::Session("variant-b".into())) },
            AgentEvent::Scoped { index: 0, event: Box::new(AgentEvent::Done("end_turn".into())) },
            AgentEvent::Scoped { index: 1, event: Box::new(AgentEvent::Done("end_turn".into())) },
        ]);
        sync(&mut app, &egui::Context::default());
        assert!(!app.ui.agent.busy);
        app.ui.agent.draft = "Polir ambas".into();
        send_prompt(&mut app);
        assert_eq!(sent.lock().unwrap()[2].resumes, [Some("variant-a".into()), Some("variant-b".into())]);
    }

    #[test]
    fn attached_agent_activity_targets_the_current_document_and_adds_preview() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent, events }));
        app.ui.agent.history_loaded = true;
        app.ui.agent.draft = "Confira o frame".into();
        send_prompt(&mut app);
        let uid = app.session.ds().unwrap().uid;
        let ctx = egui::Context::default();
        let page = app.session.ds().unwrap().page;
        assert!(
            crate::control::handle(
                &mut app,
                &ctx,
                "agent.activity",
                &json!({
                    "__agent_document": uid + 1, "phase":"done", "tool":"get_node", "ids":[page.0]
                })
            )
            .is_err()
        );
        crate::control::handle(
            &mut app,
            &ctx,
            "agent.activity",
            &json!({
                "__agent_document": uid, "phase":"done", "tool":"get_node", "ids":[page.0]
            }),
        )
        .unwrap();
        assert_eq!(app.ui.agent.presence.target, Some(page));
        assert_eq!(app.ui.agent.presence.pulses.len(), 1);
        crate::control::handle(
            &mut app,
            &ctx,
            "agent.activity",
            &json!({
                "__agent_document": uid, "phase":"done", "tool":"screenshot", "preview":"aGVsbG8="
            }),
        )
        .unwrap();
        assert_eq!(app.ui.agent.conversations[0].messages[1].previews.len(), 1);
    }

    #[test]
    fn agent_frame_edit_starts_a_transient_shimmer() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent, events }));
        let frame =
            app.session.execute("node.create", &json!({"type":"frame", "x":140, "y":90, "width":320, "height":240})).unwrap()["id"].as_u64().unwrap();
        app.ui.agent.history_loaded = true;
        app.ui.agent.draft = "Ajuste o frame".into();
        send_prompt(&mut app);
        let uid = app.session.ds().unwrap().uid;
        let ctx = egui::Context::default();
        crate::control::handle(&mut app, &ctx, "agent.activity", &json!({"__agent_document":uid,"phase":"start","tool":"get_node","ids":[frame]}))
            .unwrap();
        assert!(app.ui.agent.presence.shimmers[0].ongoing);
        crate::control::handle(&mut app, &ctx, "agent.activity", &json!({"__agent_document":uid,"phase":"done","tool":"create_node","ids":[frame]}))
            .unwrap();
        assert_eq!(app.ui.agent.presence.target, Some(NodeId(frame)));
        assert_eq!(app.ui.agent.presence.shimmers.len(), 1);
        assert_eq!(app.ui.agent.presence.shimmers[0].id, NodeId(frame));
        assert!(!app.ui.agent.presence.shimmers[0].ongoing, "the shimmer finishes after the tool completes");
    }

    #[test]
    fn parallel_agent_preview_attaches_to_its_own_response() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent, events }));
        app.ui.agent.history_loaded = true;
        app.ui.agent.agent_count = 2;
        app.ui.agent.draft = "Confira o frame".into();
        send_prompt(&mut app);
        let uid = app.session.ds().unwrap().uid;
        crate::control::handle(
            &mut app,
            &egui::Context::default(),
            "agent.activity",
            &json!({"__agent_document":uid,"__agent_index":0,"phase":"done","tool":"screenshot","preview":"aGVsbG8="}),
        )
        .unwrap();
        assert_eq!(app.ui.agent.conversations[0].messages[1].previews.len(), 1);
        assert!(app.ui.agent.conversations[0].messages[2].previews.is_empty());
    }

    #[test]
    fn selected_nodes_are_context_refs_and_excluding_one_keeps_canvas_selection() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events }));
        app.ui.agent.history_loaded = true;
        let first = app.session.execute("node.create", &json!({"type":"rectangle", "name":"First"})).unwrap()["id"].as_u64().unwrap();
        let second = app.session.execute("node.create", &json!({"type":"rectangle", "name":"Second"})).unwrap()["id"].as_u64().unwrap();
        app.session.execute("select.set", &json!({"ids":[first,second]})).unwrap();
        sync(&mut app, &egui::Context::default());
        app.ui.agent.excluded_selection.push(NodeId(first));
        app.ui.agent.draft = "Ajuste o item".into();
        send_prompt(&mut app);
        assert_eq!(app.session.ds().unwrap().selection, vec![NodeId(first), NodeId(second)]);
        assert_eq!(app.ui.agent.conversations[0].messages[0].selected_nodes.len(), 1);
        assert_eq!(app.ui.agent.conversations[0].messages[0].selected_nodes[0].id, NodeId(second));
        assert!(sent.lock().unwrap()[0].text.contains(&NodeId(second).to_string()));
        assert!(!sent.lock().unwrap()[0].text.contains(&format!("IDs selecionados: {}", NodeId(first))));
    }

    #[test]
    fn png_attachment_is_sent_as_a_separate_acp_image_and_can_be_retried() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = LizardApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events: events.clone() }));
        app.ui.agent.history_loaded = true;
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
        attach_image(&mut app, "test.png".into(), png.into_inner()).unwrap();
        send_prompt(&mut app);
        assert_eq!(sent.lock().unwrap()[0].images.len(), 1);
        assert_eq!(sent.lock().unwrap()[0].images[0].mime, "image/png");
        assert_eq!(app.ui.agent.conversations[0].messages[0].attached_images, ["test.png"]);
        events.lock().unwrap().push(AgentEvent::Error("unsupported".into()));
        sync(&mut app, &egui::Context::default());
        assert_eq!(app.ui.agent.images.len(), 1, "failed sends keep the image in the composer");
    }

    #[test]
    fn mcp_edits_are_tied_to_the_document_and_undo_entries_are_identified() {
        let mut app = LizardApp::new(Session::new(), Box::new(NoServices));
        let uid = app.session.ds().unwrap().uid;
        let ctx = egui::Context::default();
        let edit = json!({"__agent_document":uid,"command":"node.create","params":{"type":"frame","width":200,"height":100}});
        crate::control::handle(&mut app, &ctx, "engine.execute", &edit).unwrap();
        assert!(app.session.ds().unwrap().history.undo.last().unwrap().agent);
        let denied = json!({"__agent_document":uid + 1,"command":"node.create","params":{"type":"frame"}});
        assert!(crate::control::handle(&mut app, &ctx, "engine.execute", &denied).is_err());
        assert_eq!(app.session.ds().unwrap().history.undo.len(), 1);
        app.session.execute("node.create", &json!({"type":"rectangle"})).unwrap();
        assert!(!app.session.ds().unwrap().history.undo.last().unwrap().agent);
    }

    #[test]
    fn stopped_agent_cannot_apply_a_queued_edit() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut app = LizardApp::new(Session::new(), Box::new(NoServices));
        let uid = app.session.ds().unwrap().uid;
        let guard = Arc::new(AtomicBool::new(true));
        let (reply, received) = std::sync::mpsc::channel();
        app.control_queue.push(crate::ControlRequest {
            id: json!(1),
            method: "engine.execute".into(),
            params: json!({"__agent_document":uid,"command":"node.create","params":{"type":"frame"}}),
            reply,
            cancelled: Default::default(),
            agent_guard: Some(guard.clone()),
        });
        guard.store(false, Ordering::Release);
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |_ui| crate::control::drain(&mut app, &ctx));
        output.textures_delta.clear();
        assert!(received.try_recv().unwrap().result.is_err());
        assert!(app.session.ds().unwrap().history.undo.is_empty());
    }
}
