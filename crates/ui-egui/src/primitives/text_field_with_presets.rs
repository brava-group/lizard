//! An editable text field with a joined menu of suggested values.

use egui::{CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::{CONTROL_CHEVRON_SIZE, ControlWidth, Menu, MenuItem};
use crate::a11y;
use crate::i18n::tr;
use crate::icons;
use crate::theme::{Tokens, medium};
use crate::widgets;

/// A text field whose trailing chevron opens presets without taking clicks from the editor.
pub struct TextFieldWithPresets<'a> {
    id: Id,
    value: &'a str,
    options: &'a [&'a str],
    rename_action: bool,
}

impl<'a> TextFieldWithPresets<'a> {
    pub fn new(id: Id, value: &'a str, options: &'a [&'a str]) -> Self {
        Self { id, value, options, rename_action: false }
    }

    /// Add a menu command that selects the current value for typing a replacement.
    pub fn rename_action(mut self) -> Self {
        self.rename_action = true;
        self
    }

    /// Return a typed value or a selected preset. The owning panel applies the change.
    pub fn show(self, ui: &mut Ui, width: ControlWidth) -> Option<String> {
        let t = Tokens::get(ui.ctx());
        let natural = ui.painter().layout_no_wrap(self.value.to_owned(), medium(11.0), t.text).size().x + 42.0;
        let width = width.resolve(ui, natural.max(72.0)).max(48.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
        let arrow_rect = Rect::from_min_max(pos2(rect.right() - 24.0, rect.top()), rect.max);
        let text_rect = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 2.0), pos2(arrow_rect.left() - 4.0, rect.bottom() - 2.0));
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
        ui.painter().vline(arrow_rect.left(), rect.y_range(), Stroke::new(1.0, t.border));

        let arrow = ui.interact(arrow_rect, self.id.with("presets"), Sense::click());
        a11y::combo(&arrow, tr("Presets"), self.value, a11y::menu_open(&arrow));
        let changed = widgets::text_field_inline_without_ring(ui, self.id, self.value, text_rect, medium(11.0));
        icons::paint_compact_chevron(
            ui.painter(),
            Rect::from_center_size(arrow_rect.center(), vec2(CONTROL_CHEVRON_SIZE, CONTROL_CHEVRON_SIZE)),
            t.icon,
        );
        let focused = ui.memory(|memory| memory.has_focus(self.id.with("te")));
        if focused || arrow.has_focus() {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
        } else if ui.rect_contains_pointer(rect) {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.border), StrokeKind::Inside);
        }

        let mut picked = None;
        Menu::popup(&arrow).width(width.max(120.0)).show(|ui| {
            for option in self.options {
                if MenuItem::new(option).checked(*option == self.value).show(ui).clicked() {
                    picked = Some((*option).to_owned());
                    ui.close();
                }
            }
            if self.rename_action {
                Menu::separator(ui, None);
                if MenuItem::new(tr("Rename...")).show(ui).clicked() {
                    ui.memory_mut(|memory| memory.request_focus(self.id.with("te")));
                    ui.close();
                }
            }
        });
        changed.or(picked)
    }
}
