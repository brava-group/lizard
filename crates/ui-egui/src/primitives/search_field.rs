//! Search input shared by overlay compositions.

use egui::{Align, CornerRadius, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};

use super::ControlWidth;
use crate::a11y;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, regular};

pub struct SearchField<'a> {
    id: Id,
    hint: &'a str,
    width: ControlWidth,
    compact: bool,
    clearable: bool,
    ghost: bool,
}

impl<'a> SearchField<'a> {
    pub fn new(id: Id, hint: &'a str) -> Self {
        Self { id, hint, width: ControlWidth::Fill, compact: false, clearable: false, ghost: false }
    }

    pub fn width(mut self, width: ControlWidth) -> Self {
        self.width = width;
        self
    }

    /// A 24-point search field for dialog headers and toolbars.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn clearable(mut self) -> Self {
        self.clearable = true;
        self
    }

    /// Keep the surrounding panel surface visible behind the search field.
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }

    pub fn show(self, ui: &mut Ui, text: &mut String) -> Response {
        let t = Tokens::get(ui.ctx());
        let width = self.width.resolve(ui, 240.0);
        let height = if self.compact { 24.0 } else { 32.0 };
        let icon_size = if self.compact { 12.0 } else { 16.0 };
        let font_size = if self.compact { 11.0 } else { 13.0 };
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
        if !self.ghost {
            ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
        }
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(pos2(rect.left() + if self.compact { 12.0 } else { 16.0 }, rect.center().y), vec2(icon_size, icon_size)),
            Icon::Search,
            t.text_tertiary,
        );
        let show_clear = self.clearable && !text.is_empty();
        let edit_rect = egui::Rect::from_min_max(
            pos2(rect.left() + if self.compact { 24.0 } else { 36.0 }, rect.top()),
            pos2(
                rect.right()
                    - if show_clear {
                        32.0
                    } else if self.compact {
                        8.0
                    } else {
                        16.0
                    },
                rect.bottom(),
            ),
        );
        let mut child = ui.new_child(UiBuilder::new().max_rect(edit_rect));
        let mut response = child.add(
            egui::TextEdit::singleline(text)
                .id(self.id)
                .frame(egui::Frame::NONE)
                .font(regular(font_size))
                .hint_text(egui::RichText::new(self.hint).font(regular(font_size)).color(t.text_tertiary))
                .desired_width(edit_rect.width())
                .vertical_align(Align::Center)
                .min_size(vec2(0.0, height)),
        );
        if show_clear {
            let clear = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(18.0, 18.0));
            let action = ui.interact(clear, self.id.with("clear"), Sense::click()).on_hover_text(tr("Clear search"));
            ui.painter().circle_stroke(clear.center(), 7.5, Stroke::new(1.0, t.icon_secondary));
            icons::paint(ui.painter(), Rect::from_center_size(clear.center(), vec2(10.0, 10.0)), Icon::Close, t.icon_secondary);
            if action.clicked() {
                text.clear();
                response.mark_changed();
                response.request_focus();
            }
        }
        a11y::text(&response, self.hint, text);
        if response.has_focus() {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
        } else if !self.ghost && ui.rect_contains_pointer(rect) {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.field_hover_border), StrokeKind::Inside);
        }
        response
    }
}
