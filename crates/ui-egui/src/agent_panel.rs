//! The Agent sidebar and its conversation state.

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
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
    pub tools: Vec<ChatTool>,
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
                if let Some(message) = active.and_then(|chat| chat.messages.last_mut()) {
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

fn send_prompt(app: &mut FigApp) {
    let prompt = app.ui.agent.draft.trim().to_string();
    if prompt.is_empty() || app.ui.agent.busy {
        return;
    }
    let Ok(ds) = app.session.ds() else {
        app.toast("Abra um documento para conversar com o agente.");
        return;
    };
    let document = ds.uid;
    let undo_before = ds.history.undo.len();
    let selection: Vec<_> = ds.selection.iter().map(ToString::to_string).collect();
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
    match app.services.agent_send(app.ui.agent.agent, context, document) {
        Ok(()) => {
            if active.is_none() {
                app.ui.agent.conversations.push(Conversation {
                    id,
                    document: key,
                    title: prompt.chars().take(42).collect(),
                    agent: app.ui.agent.agent,
                    messages: Vec::new(),
                });
            }
            if let Some(chat) = app.ui.agent.conversations.iter_mut().find(|chat| chat.id == id) {
                chat.messages.push(ChatMessage { role: "user".into(), text: prompt, ..Default::default() });
                chat.messages.push(ChatMessage { role: "assistant".into(), undo_before, ..Default::default() });
            }
            app.ui.agent.active = Some(id);
            app.ui.agent.runtime_conversation = Some(id);
            app.ui.agent.runtime_document = Some(document);
            app.ui.agent.draft.clear();
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

pub fn show(app: &mut FigApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let width = full.width();
    let text = t.text;
    let secondary = if t.dark { MUTED } else { t.text_secondary };
    let dim = if t.dark { DIM } else { t.text_tertiary };
    let panel = t.panel;
    let border = t.border;
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

    let middle_bottom = (full.max.y - 156.0).max(header_bottom);
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
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.set_width(transcript.width());
            for (index, message) in chat.messages.iter().enumerate() {
                if message.role == "user" {
                    ui.add_space(12.0);
                    egui::Frame::new().fill(t.hover).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                        ui.set_max_width(transcript.width() - 12.0);
                        ui.label(egui::RichText::new(&message.text).size(13.0).color(text));
                    });
                } else {
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new(chat.agent.label()).size(11.0).strong().color(BLUE));
                    ui.add_space(4.0);
                    if message.text.is_empty() && app.ui.agent.busy {
                        ui.spinner();
                    } else {
                        ui.label(egui::RichText::new(&message.text).size(13.0).color(text));
                    }
                    for tool in &message.tools {
                        ui.add_space(5.0);
                        ui.label(egui::RichText::new(format!("{} · {}", tool.title, tool.status)).size(11.0).color(secondary));
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
    let composer = Rect::from_min_size(pos2(full.min.x + 16.0, full.max.y - 146.0), vec2((width - 32.0).max(0.0), 130.0));
    ui.painter().rect(composer, CornerRadius::same(13), panel, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let input = Rect::from_min_max(pos2(composer.min.x + 16.0, composer.min.y + 16.0), pos2(composer.max.x - 16.0, composer.min.y + 76.0));
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
    hit(ui, "agent_context", add, "A seleção atual é enviada como contexto");
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
        if app.ui.agent.busy || !app.ui.agent.draft.trim().is_empty() { BLUE_BG } else { Color32::from_rgb(0x75, 0x75, 0x75) },
    );
    if app.ui.agent.busy {
        ui.painter().rect_filled(Rect::from_center_size(send.center(), vec2(8.0, 8.0)), 1.0, Color32::WHITE);
    } else {
        glyph(ui, send, AgentIcon::Send, vec2(10.007, 13.001), Color32::WHITE);
    }
    let enter = input_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if send_resp.clicked() || enter {
        if app.ui.agent.busy {
            stop_chat(app);
        } else {
            send_prompt(app);
        }
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
    use figcraft_engine::Session;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct FakeAgent {
        sent: Arc<Mutex<Vec<String>>>,
        events: Arc<Mutex<Vec<AgentEvent>>>,
    }

    impl Services for FakeAgent {
        fn agent_send(&mut self, _agent: AgentKind, prompt: String, _document: u64) -> Result<(), String> {
            self.sent.lock().unwrap().push(prompt);
            Ok(())
        }

        fn agent_poll(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
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
        assert!(sent.lock().unwrap()[0].contains("Crie um frame"));
        events.lock().unwrap().extend([AgentEvent::Text("Frame criado.".into()), AgentEvent::Done("end_turn".into())]);
        sync(&mut app, &egui::Context::default());
        assert!(!app.ui.agent.busy);
        assert_eq!(app.ui.agent.conversations[0].messages[1].text, "Frame criado.");
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
