//! Compact Boolean switch for standalone controls and table cells.

use egui::{Align2, Color32, CornerRadius, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::a11y;
use crate::theme::{Tokens, regular};

pub struct Switch<'a> {
    id: Id,
    checked: bool,
    label: &'a str,
}

impl<'a> Switch<'a> {
    pub fn new(id: Id, checked: bool, label: &'a str) -> Self {
        Self { id, checked, label }
    }

    /// Allocate the control at its natural size. The caller owns the Boolean value.
    pub fn show(self, ui: &mut Ui) -> Response {
        let text_width = ui.painter().layout_no_wrap(self.label.to_owned(), regular(11.0), Tokens::get(ui.ctx()).text).size().x;
        let width = if self.label.is_empty() { 32.0 } else { 40.0 + text_width };
        let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
        self.show_at(ui, rect)
    }

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let t = Tokens::get(ui.ctx());
        let response = ui.interact(rect, self.id, Sense::click());
        a11y::toggle(&response, self.label, self.checked);
        let duration = ui.style().animation_time.min(0.16);
        let progress = ui.ctx().animate_bool_with_time(self.id.with("checked"), self.checked, duration);
        let track = Rect::from_min_size(pos2(rect.min.x, rect.center().y - 8.0), vec2(32.0, 16.0));
        let track_color = Color32::from(egui::lerp(egui::Rgba::from(t.field)..=egui::Rgba::from(t.accent), progress));
        let thumb_color = Color32::from(egui::lerp(egui::Rgba::from(t.text)..=egui::Rgba::from(t.accent_on), progress));
        ui.painter().rect_filled(track, CornerRadius::same(8), track_color);
        ui.painter().rect_stroke(track, CornerRadius::same(8), Stroke::new(1.0, t.border), StrokeKind::Inside);
        if response.has_focus() {
            ui.painter().rect_stroke(track.expand(2.0), CornerRadius::same(10), Stroke::new(1.0, t.focus), StrokeKind::Outside);
        }
        ui.painter().circle_filled(pos2(track.left() + egui::lerp(9.0..=23.0, progress), track.center().y), 6.0, thumb_color);
        ui.painter().text(pos2(track.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, self.label, regular(11.0), t.text);
        response
    }
}
