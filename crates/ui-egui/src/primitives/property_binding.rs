//! A component property bound to a layer value. The parent owns navigation and detach commands.

use egui::{Align2, CornerRadius, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

use crate::a11y;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::primitives::{Button, PropertyControllerKind};
use crate::theme::{Tokens, regular};
use crate::widgets;

pub struct PropertyBinding<'a> {
    kind: PropertyControllerKind,
    name: &'a str,
}

pub struct PropertyBindingResponse {
    pub row: Response,
    pub locate_clicked: bool,
    pub detach_clicked: bool,
}

impl<'a> PropertyBinding<'a> {
    pub fn new(kind: PropertyControllerKind, name: &'a str) -> Self {
        Self { kind, name }
    }

    pub fn show(self, ui: &mut Ui) -> PropertyBindingResponse {
        let t = Tokens::get(ui.ctx());
        let width = ui.available_width();
        let (outer, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
        let card = Rect::from_min_max(
            pos2(outer.left() + widgets::PAD, outer.center().y - widgets::FIELD_H / 2.0),
            pos2(outer.right() - 76.0, outer.center().y + widgets::FIELD_H / 2.0),
        );
        let id = ui.id().with(("property_binding", self.name));
        let row = ui.interact(card, id, Sense::click());
        a11y::button(&row, self.name);
        ui.painter().rect_filled(card, CornerRadius::same(5), t.component_row);
        self.kind.paint(ui.painter(), Rect::from_center_size(pos2(card.left() + 13.0, card.center().y), vec2(16.0, 16.0)), t.text);
        let text_rect = Rect::from_min_max(pos2(card.left() + 28.0, card.top()), pos2(card.right() - 8.0, card.bottom()));
        let name = widgets::elide_font(ui, self.name, text_rect.width(), regular(11.0));
        ui.painter().text(pos2(text_rect.left(), card.center().y), Align2::LEFT_CENTER, name, regular(11.0), t.text);

        let locate = Rect::from_center_size(pos2(outer.right() - 48.0, outer.center().y), vec2(24.0, 24.0));
        let detach = Rect::from_center_size(pos2(outer.right() - 20.0, outer.center().y), vec2(24.0, 24.0));
        let locate_response = ui.interact(locate, id.with("locate"), Sense::click()).on_hover_text(tr("Go to property"));
        a11y::button(&locate_response, tr("Go to property"));
        if locate_response.hovered() {
            ui.painter().rect_filled(locate, CornerRadius::same(4), t.hover);
        }
        ui.painter().circle_stroke(locate.center(), 7.0, Stroke::new(1.3, t.icon));
        ui.painter().circle_stroke(locate.center(), 3.5, Stroke::new(1.3, t.icon));
        ui.painter().circle_filled(locate.center(), 1.2, t.icon);
        let detach_clicked = Button::icon(Icon::LinkOff, tr("Detach property")).icon_size(22.0).show_at(ui, id.with("detach"), detach).clicked();
        PropertyBindingResponse { row, locate_clicked: locate_response.clicked(), detach_clicked }
    }
}
