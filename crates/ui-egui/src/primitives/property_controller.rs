//! Compact summary of one component property. The parent owns its edit commands.

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::a11y;
use crate::theme::{Tokens, regular, semibold};
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

    /// Geometry follows the property glyphs in the mask-example reference document.
    pub fn paint(self, painter: &Painter, rect: Rect, ink: Color32) {
        let center = rect.center();
        let at = |x: f32, y: f32| -> Pos2 { pos2(center.x + x, center.y + y) };
        let stroke = Stroke::new(1.3, ink);
        match self {
            Self::Variant | Self::InstanceSwap => {
                painter.add(egui::Shape::line(
                    vec![
                        at(0.0, -6.0),
                        at(0.7, -5.7),
                        at(5.7, -0.7),
                        at(6.0, 0.0),
                        at(5.7, 0.7),
                        at(0.7, 5.7),
                        at(0.0, 6.0),
                        at(-0.7, 5.7),
                        at(-5.7, 0.7),
                        at(-6.0, 0.0),
                        at(-5.7, -0.7),
                        at(-0.7, -5.7),
                        at(0.0, -6.0),
                    ],
                    stroke,
                ));
            }
            Self::Text => {
                painter.rect_stroke(Rect::from_center_size(center, vec2(10.0, 10.0)), CornerRadius::same(1), stroke, StrokeKind::Inside);
                painter.line_segment([at(-2.5, -1.8), at(2.5, -1.8)], stroke);
                painter.line_segment([at(0.0, -1.8), at(0.0, 2.5)], stroke);
            }
            Self::Boolean => {
                painter.add(egui::Shape::line(
                    vec![
                        at(-6.0, 0.0),
                        at(-4.8, -2.5),
                        at(-2.5, -3.7),
                        at(2.5, -3.7),
                        at(4.8, -2.5),
                        at(6.0, 0.0),
                        at(4.8, 2.5),
                        at(2.5, 3.7),
                        at(-2.5, 3.7),
                        at(-4.8, 2.5),
                        at(-6.0, 0.0),
                    ],
                    stroke,
                ));
                painter.circle_stroke(center, 1.5, stroke);
            }
            Self::Slot => {
                painter.rect_stroke(Rect::from_center_size(center, vec2(10.0, 10.0)), CornerRadius::same(1), stroke, StrokeKind::Inside);
                painter.line_segment([at(-2.0, 0.0), at(2.0, 0.0)], stroke);
                painter.line_segment([at(0.0, -2.0), at(0.0, 2.0)], stroke);
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

    pub fn show(self, ui: &mut Ui) -> Response {
        let t = Tokens::get(ui.ctx());
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let right = outer.right() - widgets::PAD - if self.unused { 26.0 } else { 0.0 };
        let card = Rect::from_min_max(pos2(outer.left() + widgets::PAD, outer.top() + 2.0), pos2(right, outer.bottom() - 6.0));
        let response = ui.interact(card, ui.id().with(("property_controller", self.name)), Sense::click_and_drag());
        a11y::button(&response, self.name);
        ui.painter().rect_filled(card, CornerRadius::same(5), if response.hovered() || self.selected { t.hover } else { t.field });
        self.kind.paint(ui.painter(), Rect::from_center_size(pos2(card.left() + 13.0, card.center().y), vec2(14.0, 14.0)), t.icon);
        let text_x = card.left() + 27.0;
        let available = (card.right() - text_x - 8.0).max(0.0);
        let name = widgets::elide_font(ui, self.name, if self.value.is_some() { available * 0.6 } else { available }, regular(11.0));
        let name_galley = ui.painter().layout_no_wrap(name, regular(11.0), t.text);
        ui.painter().galley(pos2(text_x, card.center().y - name_galley.size().y / 2.0), name_galley.clone(), t.text);
        if let Some(value) = self.value {
            let value_x = text_x + name_galley.size().x;
            let shown = widgets::elide_font(ui, &format!(" · {value}"), (card.right() - value_x - 8.0).max(0.0), regular(11.0));
            ui.painter().text(pos2(value_x, card.center().y), Align2::LEFT_CENTER, shown, regular(11.0), t.text_secondary);
        }
        if self.unused {
            let icon = Rect::from_center_size(pos2(outer.right() - widgets::PAD - 12.0, card.center().y), vec2(18.0, 18.0));
            let warning = ui.interact(icon, ui.id().with(("property_unused", self.name)), Sense::hover());
            warning.on_hover_text(crate::i18n::tr("This property is not used in the component"));
            ui.painter().circle_stroke(icon.center(), 6.0, Stroke::new(1.0, t.icon_secondary));
            ui.painter().text(icon.center(), Align2::CENTER_CENTER, "!", semibold(8.5), t.icon_secondary);
        }
        response
    }
}
