//! Unboxed reference to another variable inside a table cell.

use egui::{Align2, CornerRadius, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::a11y;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, regular};

pub struct AliasValue<'a> {
    id: Id,
    name: &'a str,
    editable: bool,
}

impl<'a> AliasValue<'a> {
    pub fn new(id: Id, name: &'a str) -> Self {
        Self { id, name, editable: true }
    }

    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Response {
        let t = Tokens::get(ui.ctx());
        let response = ui.interact(rect, self.id, if self.editable { Sense::click() } else { Sense::hover() });
        if self.editable {
            a11y::named_button(&response, self.name);
        }
        if self.editable && response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
        }
        if self.editable && response.has_focus() {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
        }
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(rect.left() + 8.0, rect.center().y), vec2(12.0, 12.0)),
            Icon::Variable,
            t.icon_secondary,
        );
        let text_rect = Rect::from_min_max(pos2(rect.left() + 20.0, rect.top()), rect.max);
        ui.painter().with_clip_rect(text_rect).text(text_rect.left_center(), Align2::LEFT_CENTER, self.name, regular(11.0), t.text);
        response
    }
}
