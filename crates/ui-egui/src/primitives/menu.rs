//! Selection menu chrome and rows shared by property popovers and the component catalog.

use egui::{
    Area, Color32, Context, CornerRadius, Id, InnerResponse, Painter, Popup, PopupCloseBehavior, Pos2, Rect, RectAlign, Response, Sense, Stroke, Ui,
    pos2, vec2,
};

use super::Surface;
use crate::a11y;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, medium, regular};

const MIN_WIDTH: f32 = 96.0;
const ROW_HEIGHT: f32 = 24.0;
const ITEM_INSET: f32 = 8.0;
const LABEL_INSET: f32 = 16.0;

/// A menu's anchored or context-menu popup. Both use the same surface and dismissal behavior.
pub struct Menu;

/// A menu positioned by its caller, for nested command menus and searchable pickers.
/// Their interaction and dismissal can depend on parent menu state.
pub struct MenuArea {
    area: Area,
}

impl MenuArea {
    /// Place an independent overlay above foreground canvas feedback when needed.
    pub fn order(mut self, order: egui::Order) -> Self {
        self.area = self.area.order(order);
        self
    }

    pub fn constrain(mut self, constrain: bool) -> Self {
        self.area = self.area.constrain(constrain);
        self
    }

    pub fn show<R>(self, ctx: &Context, content: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        self.area.show(ctx, content)
    }
}

/// Popup builder that paints the top inset highlight after its contents are measured.
pub struct MenuPopup {
    popup: Popup<'static>,
}

impl MenuPopup {
    /// Use the shared popup lifecycle with an externally controlled selection menu.
    pub fn open_memory(mut self, open: bool) -> Self {
        self.popup = self.popup.open_memory(Some(open.into()));
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.popup = self.popup.width(width);
        self
    }

    pub fn align(mut self, align: RectAlign) -> Self {
        self.popup = self.popup.align(align);
        self
    }

    pub fn at_position(mut self, position: Pos2) -> Self {
        self.popup = self.popup.at_position(position);
        self
    }

    /// Keep an overlaid selection row at its intended position near a screen edge.
    pub fn without_alignment_fallback(mut self) -> Self {
        self.popup = self.popup.align_alternatives(&[]);
        self
    }

    pub fn id(mut self, id: Id) -> Self {
        self.popup = self.popup.id(id);
        self
    }

    pub fn close_behavior(mut self, behavior: PopupCloseBehavior) -> Self {
        self.popup = self.popup.close_behavior(behavior);
        self
    }

    pub fn show<R>(self, content: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
        self.popup.show(|ui| {
            let result = content(ui);
            let rect = ui.min_rect();
            if rect.width() > 26.0 {
                let frame_rect = Rect::from_min_max(pos2(rect.min.x, rect.min.y - 8.0), pos2(rect.max.x, rect.max.y + 8.0));
                let surface = Surface::menu();
                let layer_painter = ui.ctx().layer_painter(ui.layer_id());
                surface.paint_close_shadow_outside(&layer_painter, frame_rect);
                surface.paint_top_highlight(ui.painter(), frame_rect);
            }
            result
        })
    }
}

impl Menu {
    pub fn area(id: Id, position: Pos2) -> MenuArea {
        MenuArea { area: Area::new(id).fixed_pos(position).order(egui::Order::Foreground) }
    }

    pub fn popup(anchor: &Response) -> MenuPopup {
        Self::chrome(Popup::menu(anchor), anchor.rect.width())
    }

    pub fn context(anchor: &Response) -> MenuPopup {
        Self::chrome(Popup::context_menu(anchor), MIN_WIDTH)
    }

    fn chrome(popup: Popup<'static>, width: f32) -> MenuPopup {
        let frame = Surface::menu().frame(Tokens::dark());
        let popup = popup.width(width.max(MIN_WIDTH)).frame(frame).style(|style: &mut egui::Style| {
            egui::containers::menu::menu_style(style);
            style.wrap_mode = Some(egui::TextWrapMode::Extend);
            style.spacing.item_spacing.y = 0.0;
        });
        MenuPopup { popup }
    }

    /// A rule between groups, with an optional secondary group heading below it.
    pub fn separator(ui: &mut Ui, heading: Option<&str>) {
        ui.add_space(8.0);
        let width = ui.available_width().max(MIN_WIDTH);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
        ui.painter().hline(rect.x_range().shrink(16.0), rect.center().y, Stroke::new(1.0, Color32::from_white_alpha(36)));
        if let Some(heading) = heading {
            ui.add_space(8.0);
            let (label, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
            ui.painter().text(
                pos2(label.min.x + 16.0, label.center().y),
                egui::Align2::LEFT_CENTER,
                heading,
                medium(11.0),
                Color32::from_white_alpha(178),
            );
            ui.add_space(4.0);
        } else {
            ui.add_space(8.0);
        }
    }

    /// A muted, noninteractive row aligned with menu item labels.
    pub fn status(ui: &mut Ui, text: &str) {
        let t = Tokens::get(ui.ctx());
        let width = ui.available_width().max(MIN_WIDTH);
        let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        a11y::status(&response, text, "");
        ui.painter().text(pos2(rect.min.x + LABEL_INSET, rect.center().y), egui::Align2::LEFT_CENTER, text, regular(11.0), t.menu_dim);
    }
}

/// A selectable row. The caller owns the value and closes the popup after acting on a click.
pub struct MenuItem<'a> {
    label: &'a str,
    shortcut: &'a str,
    max_width: Option<f32>,
    checked: Option<bool>,
    leading: Option<Icon>,
    trailing: Option<Icon>,
    enabled: bool,
    subtle: bool,
    palette: bool,
    highlighted: bool,
    tile: Option<char>,
}

impl<'a> MenuItem<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            shortcut: "",
            max_width: None,
            checked: None,
            leading: None,
            trailing: None,
            enabled: true,
            subtle: false,
            palette: false,
            highlighted: false,
            tile: None,
        }
    }

    /// Keep a row inside a fixed-width select menu, eliding an option that does not fit.
    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = width.is_finite().then_some(width.max(1.0));
        self
    }

    pub fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Reserve the checkmark column without drawing a checkmark.
    pub fn check_column(mut self) -> Self {
        self.checked = Some(false);
        self
    }

    pub fn leading(mut self, icon: Icon) -> Self {
        self.leading = Some(icon);
        self
    }

    pub fn trailing(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn subtle(mut self, subtle: bool) -> Self {
        self.subtle = subtle;
        self
    }

    /// A 32 px action row for searchable palettes, retaining the menu item interaction API.
    pub fn palette(mut self, highlighted: bool) -> Self {
        self.palette = true;
        self.highlighted = highlighted;
        self
    }

    pub fn tile(mut self, letter: char) -> Self {
        self.tile = Some(letter);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        if self.palette {
            return self.show_palette(ui);
        }
        self.show_with(ui, 0.0, |_, _, _| {})
    }

    fn show_palette(self, ui: &mut Ui) -> Response {
        let t = Tokens::get(ui.ctx());
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), if self.enabled { Sense::click() } else { Sense::hover() });
        a11y::menu_item(&response, self.label, self.shortcut, None);
        if !self.enabled {
            a11y::disabled(&response);
        }
        if self.highlighted || (self.enabled && response.hovered()) {
            ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
        }
        let ink = t.text.gamma_multiply(if self.enabled { 1.0 } else { 0.4 });
        let mut text_x = rect.left() + 8.0;
        if let Some(letter) = self.tile {
            let tile = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(20.0, 20.0));
            ui.painter().rect_filled(tile, CornerRadius::same(4), t.hover);
            ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, letter.to_string(), regular(11.0), ink);
            text_x = rect.left() + 36.0;
        } else if let Some(icon) = self.leading {
            icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(16.0, 16.0)), icon, t.icon_secondary);
            text_x = rect.left() + 36.0;
        }
        let hint_width = if self.shortcut.is_empty() {
            0.0
        } else {
            ui.painter().layout_no_wrap(self.shortcut.to_owned(), regular(11.0), t.text_tertiary).size().x + 12.0
        };
        let text_rect = Rect::from_min_max(pos2(text_x, rect.top()), pos2((rect.right() - hint_width - 8.0).max(text_x), rect.bottom()));
        let text = crate::widgets::elide_font(ui, self.label, text_rect.width(), regular(13.0));
        ui.painter().text(pos2(text_x, rect.center().y), egui::Align2::LEFT_CENTER, text, regular(13.0), ink);
        if !self.shortcut.is_empty() {
            ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, self.shortcut, regular(11.0), t.text_tertiary);
        }
        response
    }

    /// `art` draws optional trailing preview art in the reserved right-hand space.
    pub fn show_with(self, ui: &mut Ui, art_width: f32, art: impl FnOnce(&Painter, Rect, Color32)) -> Response {
        self.show_with_parts(ui, 0.0, |_, _, _| {}, art_width, art)
    }

    /// Draws app-specific art beside the checkmark and/or at the right edge of a row.
    pub fn show_with_parts(
        self,
        ui: &mut Ui,
        leading_art_width: f32,
        leading_art: impl FnOnce(&Painter, Rect, Color32),
        trailing_art_width: f32,
        trailing_art: impl FnOnce(&Painter, Rect, Color32),
    ) -> Response {
        let t = Tokens::get(ui.ctx());
        let natural_label = ui.painter().layout_no_wrap(self.label.to_owned(), medium(11.0), Color32::PLACEHOLDER);
        let keys = (!self.shortcut.is_empty())
            .then(|| ui.painter().layout_no_wrap(crate::toolbar::pretty_shortcut(self.shortcut), regular(11.0), Color32::PLACEHOLDER));
        let check_width = if self.checked.is_some() { 20.0 } else { 0.0 };
        let icon_width = if self.leading.is_some() { 20.0 } else { 0.0 };
        let prefix_width = check_width + icon_width;
        let leading_width = prefix_width + leading_art_width.max(0.0);
        let trail_width = trailing_art_width.max(if self.trailing.is_some() { 16.0 } else { 0.0 });
        let trail_width = trail_width.max(keys.as_ref().map_or(0.0, |text| text.size().x));
        let trailing_space = if trail_width > 0.0 { 24.0 + trail_width } else { 0.0 };
        let intrinsic = LABEL_INSET * 2.0 + leading_width + natural_label.size().x + trailing_space;
        let width = self.max_width.map_or_else(|| ui.available_width().max(intrinsic.ceil()).max(MIN_WIDTH), |max| ui.available_width().min(max));
        let label = if self.max_width.is_some() {
            let available = (width - LABEL_INSET * 2.0 - leading_width - trailing_space).max(0.0);
            let shown = crate::widgets::elide_font(ui, self.label, available, medium(11.0));
            ui.painter().layout_no_wrap(shown, medium(11.0), Color32::PLACEHOLDER)
        } else {
            natural_label
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), if self.enabled { Sense::click() } else { Sense::hover() });
        a11y::menu_item(&response, self.label, self.shortcut, self.checked);
        if !self.enabled {
            a11y::disabled(&response);
        }
        a11y::own_focus(&response);
        let hovered = self.enabled && (response.hovered() || response.has_focus());
        if hovered {
            let fill = if self.subtle { t.hover } else { t.menu_hover };
            ui.painter().rect_filled(rect.shrink2(vec2(ITEM_INSET, 0.0)), CornerRadius::same(5), fill);
        }
        let ink = if hovered && !self.subtle { Color32::WHITE } else { t.menu_text };
        let ink = if self.enabled { ink } else { ink.gamma_multiply(0.4) };
        let muted = if hovered && !self.subtle { Color32::WHITE } else { t.menu_dim };
        let muted = if self.enabled { muted } else { muted.gamma_multiply(0.4) };
        if self.checked == Some(true) {
            icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.min.x + 17.0, rect.center().y), vec2(12.0, 12.0)), Icon::Check, ink);
        }
        if let Some(icon) = self.leading {
            icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.min.x + 17.0 + check_width, rect.center().y), vec2(16.0, 16.0)), icon, ink);
        }
        if leading_art_width > 0.0 {
            let art_rect = Rect::from_center_size(
                pos2(rect.min.x + LABEL_INSET + prefix_width + leading_art_width / 2.0, rect.center().y),
                vec2(leading_art_width, ROW_HEIGHT),
            );
            leading_art(ui.painter(), art_rect, ink);
        }
        ui.painter().galley(pos2(rect.min.x + LABEL_INSET + leading_width, rect.center().y - label.size().y / 2.0), label, ink);
        if let Some(keys) = keys {
            ui.painter().galley(pos2(rect.max.x - 24.0 - keys.size().x, rect.center().y - keys.size().y / 2.0), keys, muted);
        }
        if let Some(icon) = self.trailing {
            icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.max.x - 19.0, rect.center().y), vec2(12.0, 12.0)), icon, ink);
        }
        trailing_art(ui.painter(), rect.shrink2(vec2(8.0, 0.0)), ink);
        response
    }
}
