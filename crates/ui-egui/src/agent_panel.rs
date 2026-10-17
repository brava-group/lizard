//! The Agent sidebar. This is a local chat composer until an AI provider is connected.

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use crate::FigApp;
use crate::a11y;
use crate::icons::{self, Icon, NavIcon};
use crate::theme::{Tokens, regular, semibold};

const BLUE: Color32 = Color32::from_rgb(0x7C, 0xC4, 0xF8);
const BLUE_BG: Color32 = Color32::from_rgb(0x39, 0x43, 0x60);
const MUTED: Color32 = Color32::from_rgba_premultiplied(178, 178, 178, 178);
const DIM: Color32 = Color32::from_rgba_premultiplied(102, 102, 102, 102);

#[derive(Default)]
pub struct AgentUi {
    pub draft: String,
    pub history_open: bool,
    pub menu_open: bool,
    pub selected_suggestion: Option<usize>,
}

impl AgentUi {
    fn new_chat(&mut self) {
        self.draft.clear();
        self.history_open = false;
        self.menu_open = false;
        self.selected_suggestion = None;
    }
}

fn hit(ui: &mut Ui, id: &'static str, rect: Rect, tip: &'static str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(id), Sense::click()).on_hover_text(tip);
    a11y::button(&response, tip);
    response
}

fn icon(ui: &Ui, rect: Rect, kind: Icon, size: f32, color: Color32) {
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(size, size)), kind, color);
}

fn line(ui: &Ui, a: egui::Pos2, b: egui::Pos2, color: Color32) {
    ui.painter().line_segment([a, b], Stroke::new(1.0, color));
}

fn welcome_emblem(ui: &Ui, rect: Rect) {
    let p = ui.painter();
    let c = rect.center();
    let x = c.x - 8.0;
    let y = c.y - 8.0;
    let stroke = Stroke::new(1.1, BLUE);
    p.rect_stroke(Rect::from_min_size(pos2(x + 1.0, y + 10.0), vec2(6.0, 6.0)), CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
    p.circle_stroke(pos2(x + 12.8, y + 4.0), 3.2, stroke);
    p.add(egui::Shape::convex_polygon(
        vec![pos2(x + 4.0, y), pos2(x + 8.0, y + 4.0), pos2(x + 4.0, y + 8.0), pos2(x, y + 4.0)],
        Color32::TRANSPARENT,
        stroke,
    ));
    line(ui, pos2(x + 12.8, y + 10.0), pos2(x + 12.8, y + 16.0), BLUE);
    line(ui, pos2(x + 9.8, y + 13.0), pos2(x + 15.8, y + 13.0), BLUE);
}

fn suggestion_icon(ui: &Ui, rect: Rect, index: usize, color: Color32) {
    match index {
        0 => {
            icon(ui, rect, Icon::Pencil, 15.0, color);
            let c = rect.center();
            ui.painter().circle_filled(pos2(c.x - 7.0, c.y - 7.0), 1.0, color);
        }
        1 => icons::paint_nav(ui.painter(), Rect::from_center_size(rect.center(), vec2(14.0, 14.0)), NavIcon::Agent, color),
        _ => {
            let c = rect.center();
            let stroke = Stroke::new(1.0, color);
            ui.painter().rect_stroke(
                Rect::from_center_size(pos2(c.x, c.y + 1.0), vec2(13.0, 9.0)),
                CornerRadius::same(1),
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().line_segment([pos2(c.x - 3.0, c.y - 4.0), pos2(c.x - 3.0, c.y - 6.0)], stroke);
            ui.painter().line_segment([pos2(c.x + 3.0, c.y - 4.0), pos2(c.x + 3.0, c.y - 6.0)], stroke);
            ui.painter().line_segment([pos2(c.x - 3.0, c.y - 6.0), pos2(c.x + 3.0, c.y - 6.0)], stroke);
            ui.painter().line_segment([pos2(c.x - 6.0, c.y + 1.0), pos2(c.x + 6.0, c.y + 1.0)], stroke);
        }
    }
}

fn microphone(ui: &Ui, rect: Rect) {
    let c = rect.center();
    let stroke = Stroke::new(1.0, BLUE);
    ui.painter().rect_stroke(Rect::from_center_size(pos2(c.x, c.y - 2.0), vec2(5.5, 8.0)), CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
    let arc = (0..=12)
        .map(|step| {
            let angle = std::f32::consts::PI * step as f32 / 12.0;
            pos2(c.x + 5.0 * angle.cos(), c.y + 5.0 * angle.sin())
        })
        .collect();
    ui.painter().add(egui::Shape::line(arc, stroke));
    line(ui, pos2(c.x, c.y + 5.0), pos2(c.x, c.y + 7.0), BLUE);
    line(ui, pos2(c.x - 2.0, c.y + 7.0), pos2(c.x + 2.0, c.y + 7.0), BLUE);
}

fn send_arrow(ui: &Ui, rect: Rect) {
    let c = rect.center();
    let s = Stroke::new(1.0, Color32::WHITE);
    ui.painter().line_segment([pos2(c.x, c.y - 6.0), pos2(c.x, c.y + 6.0)], s);
    ui.painter().line_segment([pos2(c.x - 4.5, c.y - 1.5), pos2(c.x, c.y - 6.0)], s);
    ui.painter().line_segment([pos2(c.x, c.y - 6.0), pos2(c.x + 4.5, c.y - 1.5)], s);
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
    let c = back.center();
    let s = Stroke::new(1.0, text);
    ui.painter().line_segment([pos2(c.x + 2.5, c.y - 4.5), pos2(c.x - 2.0, c.y)], s);
    ui.painter().line_segment([pos2(c.x - 2.0, c.y), pos2(c.x + 2.5, c.y + 4.5)], s);
    if back_resp.clicked() {
        app.ui.agent.history_open = !app.ui.agent.history_open;
    }
    let title_x = full.min.x + 36.0;
    let title = if app.ui.agent.history_open { "Todos os chats" } else { "Novo chat" };
    ui.painter().text(pos2(title_x, full.min.y + 12.0), Align2::LEFT_TOP, title, semibold(13.0), text);
    ui.painter().text(pos2(title_x, full.min.y + 34.0), Align2::LEFT_TOP, "Compartilhado", regular(11.0), secondary);
    let more = Rect::from_min_size(pos2(full.max.x - 38.0, full.min.y + 12.0), vec2(24.0, 24.0));
    let more_resp = hit(ui, "agent_options", more, "Mais opções");
    if more_resp.hovered() || app.ui.agent.menu_open {
        ui.painter().rect_filled(more, CornerRadius::same(5), t.hover);
    }
    for dx in [-6.0, 0.0, 6.0] {
        ui.painter().circle_filled(pos2(more.center().x + dx, more.center().y), 1.5, text);
    }
    if more_resp.clicked() {
        app.ui.agent.menu_open = !app.ui.agent.menu_open;
    }
    let badge = Rect::from_center_size(pos2(full.max.x - 53.5, full.min.y + 24.0), vec2(19.0, 18.0));
    ui.painter().rect_stroke(badge, CornerRadius::same(5), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter().text(badge.center(), Align2::CENTER_CENTER, "IA", semibold(11.0), secondary);

    let middle_bottom = (full.max.y - 156.0).max(header_bottom);
    let middle = Rect::from_min_max(pos2(full.min.x, header_bottom), pos2(full.max.x, middle_bottom));
    let painter = ui.painter().with_clip_rect(middle);
    if app.ui.agent.history_open {
        let new_chat = Rect::from_min_size(pos2(full.min.x + 16.0, header_bottom + 16.0), vec2((width - 32.0).max(0.0), 42.0));
        let resp = hit(ui, "agent_new_chat", new_chat, "Novo chat");
        painter.rect(new_chat, CornerRadius::same(13), panel, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        icons::paint(ui.painter(), Rect::from_center_size(pos2(new_chat.min.x + 21.0, new_chat.center().y), vec2(14.0, 14.0)), Icon::Plus, text);
        painter.text(pos2(new_chat.min.x + 40.0, new_chat.center().y), Align2::LEFT_CENTER, "Novo chat", regular(13.0), text);
        painter.text(pos2(full.center().x, new_chat.max.y + 40.0), Align2::CENTER_CENTER, "Nenhum chat anterior", regular(11.0), secondary);
        if resp.clicked() {
            app.ui.agent.new_chat();
        }
    } else {
        let welcome_top = header_bottom + 16.0;
        let emblem = Rect::from_center_size(pos2(full.center().x, welcome_top + 36.0), vec2(40.0, 40.0));
        painter.circle_filled(emblem.center(), 20.0, BLUE_BG);
        welcome_emblem(ui, emblem);
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
            suggestion_icon(ui, ir, index, secondary);
            painter.text(pos2(card.min.x + 40.0, card.center().y), Align2::LEFT_CENTER, *label, regular(13.0), text);
            let cx = card.max.x - 23.0;
            let cy = card.center().y;
            let chevron = Stroke::new(1.0, dim);
            painter.line_segment([pos2(cx - 4.5, cy - 2.5), pos2(cx, cy + 2.5)], chevron);
            painter.line_segment([pos2(cx, cy + 2.5), pos2(cx + 4.5, cy - 2.5)], chevron);
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
    let _ = ui.put(
        input,
        egui::TextEdit::multiline(&mut app.ui.agent.draft)
            .id(Id::new("agent_prompt"))
            .font(regular(13.0))
            .hint_text("Solicitar alterações")
            .frame(egui::Frame::NONE)
            .desired_width(input.width())
            .desired_rows(3),
    );
    let actions_y = composer.max.y - 24.0;
    let add = Rect::from_min_size(pos2(composer.min.x + 12.0, actions_y), vec2(24.0, 24.0));
    let libraries = Rect::from_min_size(pos2(add.max.x + 8.0, actions_y), vec2(24.0, 24.0));
    let voice = Rect::from_min_size(pos2(composer.max.x - 68.0, actions_y), vec2(24.0, 24.0));
    let send = Rect::from_min_size(pos2(composer.max.x - 36.0, actions_y), vec2(24.0, 24.0));
    hit(ui, "agent_context", add, "Adicionar contexto (em breve)");
    hit(ui, "agent_libraries", libraries, "Bibliotecas (em breve)");
    hit(ui, "agent_voice", voice, "Ditar (em breve)");
    let send_resp = hit(ui, "agent_send", send, "A integração de IA ainda não está configurada");
    icon(ui, add, Icon::Plus, 12.0, text);
    icon(ui, libraries, Icon::Library, 14.0, text);
    ui.painter().circle_filled(voice.center(), 12.0, BLUE_BG);
    microphone(ui, voice);
    ui.painter().circle_filled(send.center(), 12.0, Color32::from_rgb(0x75, 0x75, 0x75));
    send_arrow(ui, send);
    if send_resp.clicked() && !app.ui.agent.draft.trim().is_empty() {
        app.toast("A integração de IA ainda não está configurada.");
    }

    if app.ui.agent.menu_open {
        let menu = Rect::from_min_size(pos2(full.max.x - 164.0, full.min.y + 42.0), vec2(150.0, 38.0));
        ui.painter().rect(menu, CornerRadius::same(6), t.menu, Stroke::new(1.0, border), egui::StrokeKind::Inside);
        let action = hit(ui, "agent_menu_new_chat", menu.shrink(3.0), "Novo chat");
        ui.painter().text(pos2(menu.min.x + 12.0, menu.center().y), Align2::LEFT_CENTER, "Novo chat", regular(12.0), text);
        if action.clicked() {
            app.ui.agent.new_chat();
        }
    }
}
