//! The Agent sidebar and its conversation state.

use base64::Engine as _;
use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use figcraft_doc::NodeId;
use serde::{Deserialize, Serialize};

use crate::FigApp;
use crate::a11y;
use crate::icons::{self, AgentIcon};
use crate::theme::{Tokens, regular, semibold};

const BLUE: Color32 = Color32::from_rgb(0x7C, 0xC4, 0xF8);
const BLUE_BG: Color32 = Color32::from_rgb(0x39, 0x43, 0x60);
const MUTED: Color32 = Color32::from_rgba_premultiplied(178, 178, 178, 178);
const DIM: Color32 = Color32::from_rgba_premultiplied(102, 102, 102, 102);

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    Codex,
    Claude,
    Gemini,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Gemini => "Gemini CLI",
        }
    }
}

#[derive(Clone, Debug)]
pub enum AgentEvent {
    Ready,
    Text(String),
    Tool { id: String, title: String, status: String },
    Done(String),
    Error(String),
    Permission { id: serde_json::Value, title: String, options: Vec<(String, String)> },
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatNodeRef {
    pub id: NodeId,
    pub name: String,
}

/// The ACP prompt keeps binary images separate from the visible text and node references.
pub struct AgentPrompt {
    pub text: String,
    pub images: Vec<AgentImage>,
}

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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Conversation {
    pub id: u64,
    pub document: String,
    pub title: String,
    pub agent: AgentKind,
    pub messages: Vec<ChatMessage>,
}

#[derive(Default)]
pub struct AgentUi {
    pub draft: String,
    pub images: Vec<DraftImage>,
    pub pending_images: Vec<DraftImage>,
    pub context_menu_open: bool,
    /// IDs hidden from this draft's context, without changing the canvas selection.
    pub excluded_selection: Vec<NodeId>,
    pub observed_selection: Vec<NodeId>,
    pub observed_document: Option<u64>,
    pub turn_started: Option<std::time::Instant>,
    pub history_open: bool,
    pub menu_open: bool,
    pub selected_suggestion: Option<usize>,
    pub agent: AgentKind,
    pub conversations: Vec<Conversation>,
    pub active: Option<u64>,
    pub busy: bool,
    pub history_loaded: bool,
    pub runtime_document: Option<u64>,
    pub runtime_conversation: Option<u64>,
    pub permission: Option<AgentPermission>,
}

fn document_key(app: &FigApp) -> String {
    app.session.ds().ok().map_or_else(|| "sem-documento".to_string(), |ds| ds.path.clone().unwrap_or_else(|| format!("untitled:{}", ds.uid)))
}

fn save_history(app: &mut FigApp) {
    if let Ok(value) = serde_json::to_value(&app.ui.agent.conversations) {
        app.services.agent_history_save(&value);
    }
}

pub fn sync(app: &mut FigApp, ctx: &egui::Context) {
    if !app.ui.agent.history_loaded {
        app.ui.agent.history_loaded = true;
        if let Ok(mut chats) = serde_json::from_value::<Vec<Conversation>>(app.services.agent_history_load()) {
            chats.truncate(100);
            for chat in &mut chats {
                chat.messages.truncate(200);
                for message in &mut chat.messages {
                    message.text.truncate(message.text.floor_char_boundary(64_000));
                }
            }
            app.ui.agent.conversations = chats;
        }
    }
    let document = app.session.ds().ok().map(|ds| ds.uid);
    if app.ui.agent.observed_document != document {
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
    for event in events {
        let active = app.ui.agent.runtime_conversation.and_then(|id| app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id));
        match event {
            AgentEvent::Ready => {}
            AgentEvent::Text(chunk) => {
                if let Some(message) = active.and_then(|chat| chat.messages.last_mut())
                    && message.text.len() < 100_000
                {
                    message.text.push_str(&chunk);
                }
            }
            AgentEvent::Tool { id, title, status } => {
                if let Some(message) = active.and_then(|chat| chat.messages.last_mut()) {
                    if let Some(tool) = message.tools.iter_mut().find(|tool| tool.id == id) {
                        tool.title = title;
                        tool.status = status;
                    } else if message.tools.len() < 100 {
                        message.tools.push(ChatTool { id, title, status });
                    }
                }
            }
            AgentEvent::Permission { id, title, options } => app.ui.agent.permission = Some(AgentPermission { id, title, options }),
            AgentEvent::Done(reason) => {
                app.ui.agent.pending_images.clear();
                if let Some(message) = active.and_then(|chat| chat.messages.last_mut()) {
                    message.work_seconds = app.ui.agent.turn_started.take().map(|started| started.elapsed().as_secs());
                    if let Ok(ds) = app.session.ds() {
                        message.undo_after = ds.history.undo.len();
                        message.revision_after = ds.revision;
                    }
                    message.stopped = reason != "end_turn";
                }
                app.ui.agent.busy = false;
                completed = true;
            }
            AgentEvent::Error(error) => {
                app.ui.agent.turn_started = None;
                if app.session.ds().ok().is_some_and(|ds| Some(ds.uid) == app.ui.agent.runtime_document) {
                    app.ui.agent.images.append(&mut app.ui.agent.pending_images);
                } else {
                    app.ui.agent.pending_images.clear();
                }
                if let Some(message) = active.and_then(|chat| chat.messages.last_mut()) {
                    if !message.text.is_empty() {
                        message.text.push_str("\n\n");
                    }
                    message.text.push_str(&format!("Erro: {error}"));
                } else {
                    app.toast(error);
                }
                app.ui.agent.busy = false;
                app.services.agent_reset();
                app.ui.agent.runtime_document = None;
                app.ui.agent.runtime_conversation = None;
                completed = true;
            }
        }
    }
    if completed {
        save_history(app);
    }
    if app.ui.agent.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

fn selected_context(app: &FigApp) -> Vec<ChatNodeRef> {
    let Ok(ds) = app.session.ds() else { return Vec::new() };
    ds.selection
        .iter()
        .filter(|id| !app.ui.agent.excluded_selection.contains(id))
        .filter_map(|id| ds.doc.node(*id).map(|node| ChatNodeRef { id: *id, name: node.name.clone() }))
        .take(32)
        .collect()
}

const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

fn attach_image(app: &mut FigApp, name: String, bytes: Vec<u8>) -> Result<(), String> {
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

fn attach_rgba_image(app: &mut FigApp, width: u32, height: u32, rgba: Vec<u8>) {
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

fn attach_clipboard_image(app: &mut FigApp) {
    let Some((width, height, rgba)) = app.services.clipboard_get_image() else {
        app.toast("Não há uma imagem na área de transferência.");
        return;
    };
    attach_rgba_image(app, width, height, rgba);
}

fn paste_image_into_composer(app: &mut FigApp, ui: &Ui) {
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
    if app.services.clipboard_get_html().is_some_and(|html| figcraft_engine::cmd::edit::design_clipboard_buffer(&html).is_some()) {
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

fn send_prompt(app: &mut FigApp) {
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
    let selection: Vec<_> = selected_nodes.iter().map(|node| node.id.to_string()).collect();
    let page = ds.page.to_string();
    let key = document_key(app);
    let active = app.ui.agent.active.filter(|id| app.ui.agent.conversations.iter().any(|chat| chat.id == *id && chat.document == key));
    let id = active.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |time| time.as_nanos() as u64));
    let prior = if app.ui.agent.runtime_conversation == Some(id) {
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
    let context = format!(
        "Você está editando um documento FigCraft. Use o servidor MCP figcraft para ler e alterar o documento por comandos. Página selecionada: {page}. IDs selecionados: {}. Confirme apenas ações realmente executadas.\n\nHistórico desta conversa:\n{prior}\n\nPedido: {prompt}",
        selection.join(", ")
    );
    let images = app
        .ui
        .agent
        .images
        .iter()
        .map(|image| AgentImage { mime: image.mime.clone(), data: base64::engine::general_purpose::STANDARD.encode(&image.bytes) })
        .collect();
    let attached_images = app.ui.agent.images.iter().map(|image| image.name.clone()).collect();
    match app.services.agent_send(app.ui.agent.agent, AgentPrompt { text: context, images }, document) {
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
                });
            }
            if let Some(chat) = app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id) {
                chat.messages.push(ChatMessage { role: "user".into(), text: prompt, selected_nodes, attached_images, ..Default::default() });
                chat.messages.push(ChatMessage { role: "assistant".into(), undo_before, ..Default::default() });
            }
            app.ui.agent.active = Some(id);
            app.ui.agent.runtime_conversation = Some(id);
            app.ui.agent.runtime_document = Some(document);
            app.ui.agent.draft.clear();
            app.ui.agent.pending_images = std::mem::take(&mut app.ui.agent.images);
            app.ui.agent.turn_started = Some(std::time::Instant::now());
            app.ui.agent.busy = true;
            save_history(app);
        }
        Err(error) => {
            app.services.agent_reset();
            app.toast(error);
        }
    }
}

impl AgentUi {
    fn new_chat(&mut self) {
        self.draft.clear();
        self.images.clear();
        self.pending_images.clear();
        self.context_menu_open = false;
        self.excluded_selection.clear();
        self.turn_started = None;
        self.history_open = false;
        self.menu_open = false;
        self.selected_suggestion = None;
        self.active = None;
        self.busy = false;
        self.permission = None;
    }
}

fn stop_chat(app: &mut FigApp) {
    app.services.agent_stop();
    app.services.agent_reset();
    if app.ui.agent.busy {
        let history = app.session.ds().ok().filter(|ds| Some(ds.uid) == app.ui.agent.runtime_document).map(|ds| (ds.history.undo.len(), ds.revision));
        if let Some(message) = app
            .ui
            .agent
            .runtime_conversation
            .and_then(|id| app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id))
            .and_then(|chat| chat.messages.last_mut())
        {
            message.stopped = true;
            if let Some((undo_after, revision_after)) = history {
                message.undo_after = undo_after;
                message.revision_after = revision_after;
            }
        }
        save_history(app);
    }
    app.ui.agent.busy = false;
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

fn reset_chat(app: &mut FigApp) {
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
        if locate.hovered() || exclude.hovered() { t.hover } else { t.menu },
        Stroke::new(1.0, t.border),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(pos2(rect.min.x + 9.0, rect.center().y), Align2::LEFT_CENTER, name, regular(11.0), t.text);
    ui.painter().text(remove.center(), Align2::CENTER_CENTER, "×", regular(14.0), t.text_secondary);
    (locate.clicked(), exclude.clicked())
}

pub fn show(app: &mut FigApp, ui: &mut Ui) {
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
    let images_height = if app.ui.agent.images.is_empty() { 0.0 } else { 34.0 };
    let composer_height = 130.0 + context_height + images_height;
    ui.painter().rect_filled(full, 0.0, panel);

    // Header: back, two-line title, IA badge, options.
    let header_bottom = full.min.y + 63.0;
    line(ui, pos2(full.min.x, header_bottom - 0.5), pos2(full.max.x, header_bottom - 0.5), border);
    let back = Rect::from_min_size(pos2(full.min.x + 8.0, full.min.y + 12.0), vec2(24.0, 24.0));
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
    ui.painter().text(pos2(title_x, full.min.y + 34.0), Align2::LEFT_TOP, "Local", regular(11.0), secondary);
    let more = Rect::from_min_size(pos2(full.max.x - 38.0, full.min.y + 12.0), vec2(24.0, 24.0));
    let more_resp = hit(ui, "agent_options", more, "Mais opções");
    if more_resp.hovered() || app.ui.agent.menu_open {
        ui.painter().rect_filled(more, CornerRadius::same(5), t.hover);
    }
    glyph(ui, more, AgentIcon::More, vec2(15.0, 3.0), text);
    if more_resp.clicked() {
        app.ui.agent.menu_open = !app.ui.agent.menu_open;
    }
    let badge = Rect::from_center_size(pos2(full.max.x - 55.5, full.min.y + 24.0), vec2(19.0, 18.0));
    ui.painter().rect_stroke(badge, CornerRadius::same(5), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter().text(badge.center(), Align2::CENTER_CENTER, "IA", semibold(11.0), secondary);

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
        let transcript = middle.shrink2(vec2(16.0, 12.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(transcript));
        let mut revert = None;
        let mut locate_node = None;
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.set_width(transcript.width());
            for (index, message) in chat.messages.iter().enumerate() {
                if message.role == "user" {
                    ui.add_space(12.0);
                    egui::Frame::new().fill(t.hover).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                        ui.set_max_width(transcript.width() - 12.0);
                        if !message.text.is_empty() {
                            ui.label(egui::RichText::new(&message.text).size(13.0).color(text));
                        }
                        for node in &message.selected_nodes {
                            if ui.small_button(format!("▣ {}", node.name)).on_hover_text("Localizar no canvas").clicked() {
                                locate_node = Some(node.id);
                            }
                        }
                        for image in &message.attached_images {
                            ui.label(egui::RichText::new(format!("▧ {image}")).size(11.0).color(secondary));
                        }
                    });
                } else {
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new(chat.agent.label()).size(11.0).strong().color(BLUE));
                    ui.add_space(4.0);
                    if !message.tools.is_empty() {
                        let label = message.work_seconds.map_or_else(|| "Trabalhando…".to_string(), |s| format!("Trabalhou por {s} s"));
                        egui::CollapsingHeader::new(egui::RichText::new(label).size(11.0).color(secondary))
                            .id_salt(("agent_work", chat.id, index))
                            .show(ui, |ui| {
                                for tool in &message.tools {
                                    let mark = if tool.status == "completed" {
                                        "✓"
                                    } else if tool.status == "failed" {
                                        "!"
                                    } else {
                                        "◌"
                                    };
                                    ui.label(egui::RichText::new(format!("{mark} {} · {}", tool.title, tool.status)).size(11.0).color(secondary));
                                }
                            });
                        ui.add_space(6.0);
                    }
                    if message.text.is_empty() && app.ui.agent.busy && index + 1 == chat.messages.len() {
                        ui.spinner();
                    } else if !message.text.is_empty() {
                        ui.label(egui::RichText::new(&message.text).size(13.0).color(text));
                    }
                    if message.stopped {
                        ui.label(egui::RichText::new("Interrompido").size(11.0).color(secondary));
                    }
                    if !app.ui.agent.busy
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
    }

    // Composer is anchored to the bottom, independently of the welcome area height.
    let composer = Rect::from_min_size(pos2(full.min.x + 16.0, full.max.y - 16.0 - composer_height), vec2((width - 32.0).max(0.0), composer_height));
    ui.painter().rect(composer, CornerRadius::same(13), panel, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    if !selected_nodes.is_empty() {
        let mut x = composer.min.x + 12.0;
        let mut shown = 0;
        for node in &selected_nodes {
            let label: String = node.name.chars().take(22).collect();
            let chip_width = (label.chars().count() as f32 * 6.2 + 42.0).clamp(72.0, 190.0);
            if x + chip_width > composer.max.x - 12.0 {
                break;
            }
            let chip = Rect::from_min_size(pos2(x, composer.min.y + 8.0), vec2(chip_width, 24.0));
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
                pos2(x + 3.0, composer.min.y + 20.0),
                Align2::LEFT_CENTER,
                format!("+{}", selected_nodes.len() - shown),
                regular(11.0),
                secondary,
            );
        }
    }
    if !app.ui.agent.images.is_empty() {
        let mut x = composer.min.x + 12.0;
        let mut remove_image = None;
        for (index, image) in app.ui.agent.images.iter_mut().enumerate() {
            if x + 146.0 > composer.max.x - 12.0 {
                break;
            }
            if image.preview.is_none()
                && let Ok(decoded) = image::load_from_memory(&image.bytes)
            {
                let thumb = decoded.thumbnail(48, 48).to_rgba8();
                let color = egui::ColorImage::from_rgba_unmultiplied([thumb.width() as usize, thumb.height() as usize], thumb.as_raw());
                image.preview = Some(ui.ctx().load_texture(format!("agent-image-{}-{index}", image.name), color, egui::TextureOptions::LINEAR));
            }
            let tile = Rect::from_min_size(pos2(x, composer.min.y + 8.0 + context_height), vec2(140.0, 24.0));
            let remove = Rect::from_min_size(pos2(tile.max.x - 24.0, tile.min.y), vec2(24.0, 24.0));
            ui.painter().rect(tile, CornerRadius::same(5), t.menu, Stroke::new(1.0, border), egui::StrokeKind::Inside);
            if let Some(preview) = &image.preview {
                ui.painter().image(
                    preview.id(),
                    Rect::from_min_size(pos2(tile.min.x + 4.0, tile.min.y + 3.0), vec2(18.0, 18.0)),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let label: String = image.name.chars().take(13).collect();
            ui.painter().text(pos2(tile.min.x + 27.0, tile.center().y), Align2::LEFT_CENTER, label, regular(11.0), text);
            let remove_resp = ui.interact(remove, ui.id().with(("agent_remove_image", index)), Sense::click()).on_hover_text("Remover imagem");
            a11y::button(&remove_resp, "Remover imagem");
            ui.painter().text(remove.center(), Align2::CENTER_CENTER, "×", regular(14.0), secondary);
            if remove_resp.clicked() {
                remove_image = Some(index);
            }
            x += 146.0;
        }
        if let Some(index) = remove_image {
            app.ui.agent.images.remove(index);
        }
    }
    let input = Rect::from_min_max(
        pos2(composer.min.x + 16.0, composer.min.y + 16.0 + context_height + images_height),
        pos2(composer.max.x - 16.0, composer.min.y + 76.0 + context_height + images_height),
    );
    paste_image_into_composer(app, ui);
    let input_resp = ui.put(
        input,
        egui::TextEdit::multiline(&mut app.ui.agent.draft)
            .id(Id::new("agent_prompt"))
            .font(regular(13.0))
            .hint_text(egui::RichText::new("Solicitar alterações").size(13.0).color(secondary))
            .frame(egui::Frame::NONE)
            .desired_width(input.width())
            .desired_rows(3),
    );
    let actions_y = composer.max.y - 36.0;
    let add = Rect::from_min_size(pos2(composer.min.x + 12.0, actions_y), vec2(24.0, 24.0));
    let libraries = Rect::from_min_size(pos2(add.max.x + 8.0, actions_y), vec2(24.0, 24.0));
    let voice = Rect::from_min_size(pos2(composer.max.x - 68.0, actions_y), vec2(24.0, 24.0));
    let send = Rect::from_min_size(pos2(composer.max.x - 36.0, actions_y), vec2(24.0, 24.0));
    if hit(ui, "agent_context", add, "Adicionar contexto").clicked() {
        app.ui.agent.context_menu_open = !app.ui.agent.context_menu_open;
    }
    hit(ui, "agent_libraries", libraries, "Bibliotecas (em breve)");
    hit(ui, "agent_voice", voice, "Ditar (em breve)");
    let send_resp = hit(ui, "agent_send", send, if app.ui.agent.busy { "Parar agente" } else { "Enviar mensagem" });
    glyph(ui, add, AgentIcon::Add, vec2(12.0, 12.0), text);
    glyph(ui, libraries, AgentIcon::Library, vec2(14.0, 11.75), text);
    ui.painter().circle_filled(voice.center(), 12.0, BLUE_BG);
    glyph(ui, voice, AgentIcon::Microphone, vec2(10.0, 14.0), BLUE);
    ui.painter().circle_filled(
        send.center(),
        12.0,
        if app.ui.agent.busy || !app.ui.agent.draft.trim().is_empty() || !selected_nodes.is_empty() || !app.ui.agent.images.is_empty() {
            BLUE_BG
        } else {
            Color32::from_rgb(0x75, 0x75, 0x75)
        },
    );
    if app.ui.agent.busy {
        ui.painter().rect_filled(Rect::from_center_size(send.center(), vec2(8.0, 8.0)), 1.0, Color32::WHITE);
    } else {
        glyph(ui, send, AgentIcon::Send, vec2(10.007, 13.001), Color32::WHITE);
    }
    let enter = input_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if send_resp.clicked() || enter {
        if app.ui.agent.busy && send_resp.clicked() {
            stop_chat(app);
        } else if !app.ui.agent.busy {
            send_prompt(app);
        }
    }

    if app.ui.agent.context_menu_open {
        let menu = Rect::from_min_size(pos2(composer.min.x + 12.0, composer.min.y - 72.0), vec2(176.0, 64.0));
        ui.painter().rect(menu, CornerRadius::same(6), t.menu, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        let choose = Rect::from_min_size(menu.min, vec2(menu.width(), 32.0));
        let paste = Rect::from_min_size(pos2(menu.min.x, menu.min.y + 32.0), vec2(menu.width(), 32.0));
        if hit(ui, "agent_attach_file", choose, "Anexar PNG ou JPEG").clicked() {
            app.ui.agent.context_menu_open = false;
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
        }
        if hit(ui, "agent_attach_clipboard", paste, "Colar imagem copiada").clicked() {
            app.ui.agent.context_menu_open = false;
            attach_clipboard_image(app);
        }
        ui.painter().text(pos2(choose.min.x + 12.0, choose.center().y), Align2::LEFT_CENTER, "Anexar PNG ou JPEG", regular(12.0), text);
        ui.painter().text(pos2(paste.min.x + 12.0, paste.center().y), Align2::LEFT_CENTER, "Colar imagem", regular(12.0), text);
    }

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

    if app.ui.agent.menu_open {
        let menu = Rect::from_min_size(pos2(full.max.x - 180.0, full.min.y + 42.0), vec2(166.0, 120.0));
        ui.painter().rect(menu, CornerRadius::same(6), t.menu, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        let new = Rect::from_min_size(menu.min, vec2(menu.width(), 30.0));
        if hit(ui, "agent_menu_new_chat", new, "Novo chat").clicked() {
            reset_chat(app);
        }
        ui.painter().text(pos2(new.min.x + 12.0, new.center().y), Align2::LEFT_CENTER, "Novo chat", regular(12.0), text);
        for (index, kind) in [AgentKind::Codex, AgentKind::Claude, AgentKind::Gemini].into_iter().enumerate() {
            let row = Rect::from_min_size(pos2(menu.min.x, menu.min.y + 30.0 + index as f32 * 30.0), vec2(menu.width(), 30.0));
            if hit(
                ui,
                match kind {
                    AgentKind::Codex => "agent_codex",
                    AgentKind::Claude => "agent_claude",
                    AgentKind::Gemini => "agent_gemini",
                },
                row,
                kind.label(),
            )
            .clicked()
            {
                reset_chat(app);
                app.ui.agent.agent = kind;
                app.ui.agent.menu_open = false;
            }
            ui.painter().text(
                pos2(row.min.x + 12.0, row.center().y),
                Align2::LEFT_CENTER,
                kind.label(),
                regular(12.0),
                if app.ui.agent.agent == kind { BLUE } else { text },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoServices, Services};
    use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
    use figcraft_engine::Session;
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
        let mut app = FigApp::new(Session::new(), Box::new(ImagePasteClipboard));
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
        let mut app = FigApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events: events.clone() }));
        app.ui.agent.history_loaded = true;
        app.ui.agent.draft = "Crie um frame".into();
        send_prompt(&mut app);
        assert!(app.ui.agent.busy);
        assert!(app.ui.agent.draft.is_empty());
        assert!(sent.lock().unwrap()[0].text.contains("Crie um frame"));
        events.lock().unwrap().extend([AgentEvent::Text("Frame criado.".into()), AgentEvent::Done("end_turn".into())]);
        sync(&mut app, &egui::Context::default());
        assert!(!app.ui.agent.busy);
        assert_eq!(app.ui.agent.conversations[0].messages[1].text, "Frame criado.");
    }

    #[test]
    fn selected_nodes_are_context_refs_and_excluding_one_keeps_canvas_selection() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut app = FigApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events }));
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
        let mut app = FigApp::new(Session::new(), Box::new(FakeAgent { sent: sent.clone(), events: events.clone() }));
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
        let mut app = FigApp::new(Session::new(), Box::new(NoServices));
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

        let mut app = FigApp::new(Session::new(), Box::new(NoServices));
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
