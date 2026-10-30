//! Compact Boolean switch for table cells.

use egui::{Align2, CornerRadius, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

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

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let t = Tokens::get(ui.ctx());
        let response = ui.interact(rect, self.id, Sense::click());
        a11y::toggle(&response, self.label, self.checked);
        let track = Rect::from_min_size(pos2(rect.min.x, rect.center().y - 8.0), vec2(32.0, 16.0));
        ui.painter().rect_filled(track, CornerRadius::same(8), if self.checked { t.accent } else { t.field });
        ui.painter().rect_stroke(track, CornerRadius::same(8), Stroke::new(1.0, t.icon_secondary), StrokeKind::Inside);
        if response.has_focus() {
            ui.painter().rect_stroke(track.expand(2.0), CornerRadius::same(10), Stroke::new(1.0, t.focus), StrokeKind::Outside);
        }
        ui.painter().circle_filled(pos2(track.left() + if self.checked { 23.0 } else { 9.0 }, track.center().y), 6.0, t.text);
        ui.painter().text(pos2(track.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, self.label, regular(11.0), t.text);
        response
    }
}
