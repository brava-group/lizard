//! Compact summary of one component property. The parent owns its edit commands.

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::a11y;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, regular};
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyControllerKind {
    Variant,
    Text,
    Boolean,
    InstanceSwap,
    Slot,
}

impl PropertyControllerKind {
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "variant" => Some(Self::Variant),
            "text" => Some(Self::Text),
            "boolean" => Some(Self::Boolean),
            "instanceSwap" => Some(Self::InstanceSwap),
            "slot" => Some(Self::Slot),
            _ => None,
        }
    }

    /// Property glyphs shared by the component panel and its creation menu.
    pub fn paint(self, painter: &Painter, rect: Rect, ink: Color32) {
        let center = rect.center();
        let at = |x: f32, y: f32| -> Pos2 { pos2(center.x + x, center.y + y) };
        match self {
            Self::Variant => {
                let size = rect.width().min(rect.height()).min(14.0);
                icons::paint(painter, Rect::from_center_size(center, vec2(size, size)), Icon::AddVariant, ink);
            }
            Self::InstanceSwap => {
                // The supplied 24×24 swap SVG: arrow feeding a circular instance.
                let scale = 0.65;
                let line = Stroke::new(1.5 * scale, ink);
                painter.circle_stroke(at(4.0 * scale, 0.0), 6.0 * scale, line);
                painter.line_segment([at(-10.0 * scale, 0.0), at(-2.0 * scale, 0.0)], line);
                painter.line_segment([at(-10.0 * scale, 0.0), at(-7.0 * scale, -3.0 * scale)], line);
                painter.line_segment([at(-10.0 * scale, 0.0), at(-7.0 * scale, 3.0 * scale)], line);
            }
            Self::Text => {
                // The supplied 24×24 text-property SVG, fitted into the row icon box.
                let scale = 0.7;
                let line = Stroke::new(1.5 * scale, ink);
                painter.rect_stroke(
                    Rect::from_center_size(center, vec2(18.0 * scale, 18.0 * scale)),
                    CornerRadius::same(1),
                    line,
                    StrokeKind::Inside,
                );
                painter.line_segment([at(-5.0 * scale, -3.0 * scale), at(-5.0 * scale, -5.0 * scale)], line);
                painter.line_segment([at(-5.0 * scale, -5.0 * scale), at(5.0 * scale, -5.0 * scale)], line);
                painter.line_segment([at(5.0 * scale, -5.0 * scale), at(5.0 * scale, -3.0 * scale)], line);
                painter.line_segment([at(0.0, -5.0 * scale), at(0.0, 5.0 * scale)], line);
                painter.line_segment([at(-2.0 * scale, 5.0 * scale), at(2.0 * scale, 5.0 * scale)], line);
            }
            Self::Boolean => {
                // The supplied 20×20 toggle SVG, scaled to the property's 14 px icon box.
                let scale = 0.8;
                painter.rect_stroke(
                    Rect::from_center_size(center, vec2(16.0 * scale, 9.6 * scale)),
                    CornerRadius::same(4),
                    Stroke::new(1.3 * scale, ink),
                    StrokeKind::Inside,
                );
                painter.circle_filled(at(3.2 * scale, 0.0), 1.6 * scale, ink);
            }
            Self::Slot => {
                // The supplied 24×24 slot SVG: dashed outer and solid inner rounded squares.
                let scale = 0.65;
                let half = 10.0 * scale;
                let radius = 6.0 * scale;
                let mut outline = Vec::with_capacity(25);
                for (cx, cy, start) in [
                    (half - radius, -half + radius, -std::f32::consts::FRAC_PI_2),
                    (half - radius, half - radius, 0.0),
                    (-half + radius, half - radius, std::f32::consts::FRAC_PI_2),
                    (-half + radius, -half + radius, std::f32::consts::PI),
                ] {
                    for step in 0..=5 {
                        let angle = start + step as f32 * std::f32::consts::FRAC_PI_2 / 5.0;
                        outline.push(at(cx + radius * angle.cos(), cy + radius * angle.sin()));
                    }
                }
                if let Some(first) = outline.first().copied() {
                    outline.push(first);
                }
                painter.extend(egui::Shape::dashed_line(&outline, Stroke::new(1.5 * scale, ink), 2.0 * scale, 2.0 * scale));
                painter.rect_stroke(
                    Rect::from_center_size(center, vec2(14.0 * scale, 14.0 * scale)),
                    CornerRadius::same(2),
                    Stroke::new(1.5 * scale, ink),
                    StrokeKind::Inside,
                );
            }
        }
    }
}

pub struct PropertyController<'a> {
    kind: PropertyControllerKind,
    name: &'a str,
    value: Option<&'a str>,
    unused: bool,
    selected: bool,
}

pub struct PropertyControllerResponse {
    pub row: Response,
    pub renamed: Option<String>,
    pub settings_clicked: bool,
    pub delete_clicked: bool,
}

impl<'a> PropertyController<'a> {
    pub fn new(kind: PropertyControllerKind, name: &'a str) -> Self {
        Self { kind, name, value: None, unused: false, selected: false }
    }

    pub fn value(mut self, value: &'a str) -> Self {
        self.value = Some(value);
        self
    }

    pub fn unused(mut self, unused: bool) -> Self {
        self.unused = unused;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn show(self, ui: &mut Ui) -> PropertyControllerResponse {
        let t = Tokens::get(ui.ctx());
        let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
        let hover_fill = Color32::from_rgb(
            mid(t.field.r(), t.surface_tertiary.r()),
            mid(t.field.g(), t.surface_tertiary.g()),
            mid(t.field.b(), t.surface_tertiary.b()),
        );
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let id = ui.id().with(("property_controller", self.name));
        let editing_id = id.with("editing");
        let editor_id = id.with("name");
        let mut editing = ui.data(|data| data.get_temp::<bool>(editing_id)).unwrap_or(false);
        let hovered = ui.input(|input| input.pointer.hover_pos().is_some_and(|pos| outer.contains(pos)));
        let actions_visible = hovered && !editing;
        let warning_width = if self.unused { 26.0 } else { 0.0 };
        let action_width = if actions_visible { 56.0 } else { 0.0 };
        let right = outer.right() - widgets::PAD - warning_width;
        let card = Rect::from_min_max(pos2(outer.left() + widgets::PAD, outer.top() + 2.0), pos2(right - action_width, outer.bottom() - 6.0));
        let row = ui.interact(card, id, Sense::click_and_drag());
        a11y::button(&row, self.name);
        if row.clicked() {
            editing = true;
            ui.memory_mut(|memory| memory.request_focus(editor_id.with("te")));
        }
        ui.painter().rect_filled(card, CornerRadius::same(5), if hovered || self.selected || editing { hover_fill } else { t.field });
        if editing {
            ui.painter().rect_stroke(card, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
        }
        self.kind.paint(ui.painter(), Rect::from_center_size(pos2(card.left() + 13.0, card.center().y), vec2(14.0, 14.0)), t.icon);
        let text_x = card.left() + 27.0;
        let mut renamed = None;
        if editing {
            let editor = Rect::from_min_max(pos2(text_x, card.top()), pos2(card.right() - 7.0, card.bottom()));
            renamed = widgets::text_field_inline_without_ring(ui, editor_id, self.name, editor, regular(11.0));
            editing = ui.memory(|memory| memory.has_focus(editor_id.with("te")));
        } else {
            let available = (card.right() - text_x - 8.0).max(0.0);
            let name = widgets::elide_font(ui, self.name, if self.value.is_some() { available * 0.6 } else { available }, regular(11.0));
            let name_galley = ui.painter().layout_no_wrap(name, regular(11.0), t.text);
            ui.painter().galley(pos2(text_x, card.center().y - name_galley.size().y / 2.0), name_galley.clone(), t.text);
            if let Some(value) = self.value {
                let value_x = text_x + name_galley.size().x;
                let shown = widgets::elide_font(ui, &format!(" · {value}"), (card.right() - value_x - 8.0).max(0.0), regular(11.0));
                ui.painter().text(pos2(value_x, card.center().y), Align2::LEFT_CENTER, shown, regular(11.0), t.text_secondary);
            }
        }
        ui.data_mut(|data| data.insert_temp(editing_id, editing));
        let mut settings_clicked = false;
        let mut delete_clicked = false;
        if actions_visible {
            let settings_rect = Rect::from_center_size(pos2(right - 36.0, card.center().y), vec2(22.0, 22.0));
            let settings = ui.interact(settings_rect, id.with("settings"), Sense::click()).on_hover_text(crate::i18n::tr("Property settings"));
            a11y::button(&settings, crate::i18n::tr("Property settings"));
            settings_clicked = settings.clicked();
            if settings.hovered() {
                ui.painter().rect_filled(settings_rect, CornerRadius::same(4), t.hover);
            }
            let center = settings_rect.center();
            let stroke = Stroke::new(1.2, t.icon);
            for (x, knob_y) in [(-3.5, 1.5), (3.5, -1.5)] {
                ui.painter().line_segment([pos2(center.x + x, center.y - 5.5), pos2(center.x + x, center.y + 5.5)], stroke);
                ui.painter().circle_filled(pos2(center.x + x, center.y + knob_y), 2.4, hover_fill);
                ui.painter().circle_stroke(pos2(center.x + x, center.y + knob_y), 2.0, stroke);
            }
            let delete_rect = Rect::from_center_size(pos2(right - 12.0, card.center().y), vec2(22.0, 22.0));
            let delete = ui.interact(delete_rect, id.with("delete"), Sense::click()).on_hover_text(crate::i18n::tr("Delete property"));
            a11y::button(&delete, crate::i18n::tr("Delete property"));
            delete_clicked = delete.clicked();
            if delete.hovered() {
                ui.painter().rect_filled(delete_rect, CornerRadius::same(4), t.hover);
            }
            ui.painter().line_segment(
                [pos2(delete_rect.center().x - 5.0, delete_rect.center().y), pos2(delete_rect.center().x + 5.0, delete_rect.center().y)],
                Stroke::new(1.3, t.icon),
            );
        }
        if self.unused {
            let icon = Rect::from_center_size(widgets::section_action_rect(outer, 0).center(), vec2(18.0, 18.0));
            let warning = ui.interact(icon, ui.id().with(("property_unused", self.name)), Sense::hover());
            warning.on_hover_text(crate::i18n::tr("This property is not used in the component"));
            let center = icon.center();
            let ink = t.icon_secondary;
            ui.painter().circle_stroke(center, 6.8, Stroke::new(1.1, ink));
            ui.painter().line_segment([pos2(center.x, center.y - 3.4), pos2(center.x, center.y + 0.7)], Stroke::new(1.1, ink));
            ui.painter().circle_filled(pos2(center.x, center.y - 3.4), 0.55, ink);
            ui.painter().circle_filled(pos2(center.x, center.y + 0.7), 0.55, ink);
            ui.painter().circle_filled(pos2(center.x, center.y + 3.4), 0.7, ink);
        }
        PropertyControllerResponse { row, renamed, settings_clicked, delete_clicked }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, RawInput, Rect, UiBuilder, pos2, vec2};

    use super::{PropertyController, PropertyControllerKind};

    fn frame(ctx: &egui::Context, events: Vec<Event>) -> Option<String> {
        let mut renamed = None;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0))), events, ..Default::default() };
        ctx.run_ui(input, |ui| {
            let mut row = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(20.0, 20.0), vec2(300.0, 40.0))));
            renamed = PropertyController::new(PropertyControllerKind::Slot, "Slot").show(&mut row).renamed;
        })
        .textures_delta
        .clear();
        renamed
    }

    fn pointer(pressed: bool) -> Event {
        Event::PointerButton { pos: pos2(70.0, 34.0), button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    #[test]
    fn clicking_a_property_selects_its_name_and_enter_commits_the_rename() {
        let ctx = egui::Context::default();
        frame(&ctx, vec![]);
        frame(&ctx, vec![Event::PointerMoved(pos2(70.0, 34.0))]);
        frame(&ctx, vec![pointer(true)]);
        frame(&ctx, vec![pointer(false)]);
        frame(&ctx, vec![]);
        frame(&ctx, vec![Event::Text("Content".into())]);
        let renamed = frame(&ctx, vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert_eq!(renamed.as_deref(), Some("Content"));
    }
}
