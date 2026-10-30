//! An unboxed text editor for values inside table cells.

use egui::{Id, Rect, Sense, Ui, pos2, vec2};

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::theme::regular;
use crate::widgets::{self, FieldEdit};

pub struct InlineField<'a> {
    id: Id,
    value: &'a str,
}

impl<'a> InlineField<'a> {
    pub fn new(id: Id, value: &'a str) -> Self {
        Self { id, value }
    }

    /// Return a committed edit; the parent owns validation and document commands.
    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> Option<String> {
        widgets::text_field_inline(ui, self.id, self.value, rect, regular(11.0))
    }

    /// Keep number scrubbing available without adding a filled field at rest.
    pub fn show_numeric_at(self, ui: &mut Ui, rect: Rect, from: f64, step: f64) -> (Option<String>, Option<FieldEdit>) {
        let id = self.id;
        let handle = Rect::from_min_max(pos2(rect.right() - 18.0, rect.top()), rect.max);
        let text_rect = Rect::from_min_max(rect.min, pos2(handle.left(), rect.bottom()));
        let edit = self.show_at(ui, text_rect);
        let response = ui.interact(handle, id.with("scrub_handle"), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            icons::paint(
                ui.painter(),
                Rect::from_center_size(handle.center(), vec2(10.0, 10.0)),
                Icon::DragHandle,
                Tokens::get(ui.ctx()).icon_secondary,
            );
        }
        let scrub = widgets::scrub(ui, id.with("numeric"), &response, Some(from), step);
        (edit, scrub)
    }
}
