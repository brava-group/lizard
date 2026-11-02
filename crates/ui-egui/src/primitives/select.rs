//! A selection control with a menu that can overlay its current value or sit beside the trigger.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, Color32, CornerRadius, Rect, RectAlign, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::{CONTROL_CHEVRON_SIZE, ControlWidth, Menu, MenuItem, MenuPopup};
use crate::a11y;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, medium, regular};
use crate::widgets;

const HEIGHT: f32 = 24.0;
const MENU_MARGIN: f32 = 8.0;
const MENU_ROW: f32 = 24.0;

/// Placement of the open options. Overlay aligns the selected row with the closed value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectPosition {
    #[default]
    Overlay,
    BottomLeft,
    BottomCenter,
    BottomRight,
    TopLeft,
    TopCenter,
    TopRight,
}

/// Visual treatment of the closed control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectVariant {
    #[default]
    Default,
    Ghost,
    Outlined,
}

/// Optional art before the selected value.
#[derive(Clone, Copy)]
pub enum SelectLead {
    Icon(Icon),
    Dot(Color32),
}

/// The caller owns the selected index and applies the value returned by [`Select::show`].
pub struct Select<'a> {
    options: &'a [&'a str],
    selected: usize,
    label: &'a str,
    width: ControlWidth,
    position: SelectPosition,
    variant: SelectVariant,
    lead: Option<SelectLead>,
    enabled: bool,
}

impl<'a> Select<'a> {
    pub fn new(options: &'a [&'a str], selected: usize) -> Self {
        Self {
            options,
            selected,
            label: "",
            width: ControlWidth::Hug,
            position: SelectPosition::Overlay,
            variant: SelectVariant::Default,
            lead: None,
            enabled: true,
        }
    }

    pub fn label(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }

    pub fn width(mut self, width: ControlWidth) -> Self {
        self.width = width;
        self
    }

    pub fn position(mut self, position: SelectPosition) -> Self {
        self.position = position;
        self
    }

    pub fn variant(mut self, variant: SelectVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn lead(mut self, lead: SelectLead) -> Self {
        self.lead = Some(lead);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Draw the trigger and options, returning an index when the selection changes.
    pub fn show(self, ui: &mut Ui) -> Option<usize> {
        let t = Tokens::get(ui.ctx());
        let current = self.options.get(self.selected).copied().unwrap_or("");
        let longest = self
            .options
            .iter()
            .map(|option| ui.painter().layout_no_wrap((*option).to_owned(), medium(11.0), Color32::WHITE).size().x)
            .fold(0.0_f32, f32::max);
        let natural = (longest + 52.0 + if self.lead.is_some() { 18.0 } else { 0.0 }).max(96.0);
        let width = self.width.resolve(ui, natural).max(72.0);
        let (rect, response) =
            ui.allocate_exact_size(vec2(width, HEIGHT), if self.enabled && !self.options.is_empty() { Sense::click() } else { Sense::hover() });
        let opened = a11y::menu_open(&response) || response.clicked();
        a11y::combo(&response, self.label, current, opened);
        if !self.enabled || self.options.is_empty() {
            a11y::disabled(&response);
        }
        self.paint_trigger(ui, rect, &response, current, t, opened);
        if !self.enabled || self.options.is_empty() {
            return None;
        }

        let popup = Self::popup_for(ui, &response, self.selected, self.options.len(), width, self.position);

        let mut changed = None;
        popup.show(|ui| {
            for (index, label) in self.options.iter().enumerate() {
                if MenuItem::new(label).checked(self.selected == index).max_width(width).show(ui).clicked() {
                    changed = Some(index);
                    ui.close();
                }
            }
        });
        changed
    }

    /// Draw just the Select trigger when the options need a custom menu (icons, sections,
    /// previews, or commands). Use [`Select::popup_for`] to place that menu.
    pub fn trigger(ui: &mut Ui, value: &str, width: f32, lead: Option<Icon>) -> Response {
        Self::trigger_with_lead(ui, value, width, lead.map(SelectLead::Icon))
    }

    pub fn trigger_with_lead(ui: &mut Ui, value: &str, width: f32, lead: Option<SelectLead>) -> Response {
        Self::trigger_with_variant(ui, value, width, lead, SelectVariant::Default)
    }

    /// Draw a trigger for a custom menu with an explicit Select appearance.
    pub fn trigger_with_variant(ui: &mut Ui, value: &str, width: f32, lead: Option<SelectLead>, variant: SelectVariant) -> Response {
        let t = Tokens::get(ui.ctx());
        let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
        a11y::combo(&response, "", value, a11y::menu_open(&response));
        let select = Self {
            options: &[],
            selected: 0,
            label: "",
            width: ControlWidth::Fixed(width),
            position: SelectPosition::Overlay,
            variant,
            lead,
            enabled: true,
        };
        select.paint_trigger(ui, rect, &response, value, t, a11y::menu_open(&response));
        let label_x = rect.left() + 8.0 + if lead.is_some() { 18.0 } else { 0.0 };
        let shown = widgets::elide(ui, value, (rect.right() - label_x - 20.0).max(0.0));
        if shown != value { response.lizard_tooltip(value) } else { response }
    }

    /// Use the Select menu placement with a composite trigger, such as a number field or
    /// a toolbar icon group. The caller can draw richer rows and keep its own selected value.
    pub fn popup_for(ui: &Ui, anchor: &Response, selected: usize, row_count: usize, width: f32, position: SelectPosition) -> MenuPopup {
        let mut popup = Menu::popup(anchor).width(width);
        if position == SelectPosition::Overlay && row_count > 0 {
            let bounds = ui.ctx().content_rect();
            let height = MENU_MARGIN * 2.0 + row_count as f32 * MENU_ROW;
            let top = anchor.rect.center().y - MENU_MARGIN - (selected.min(row_count - 1) as f32 + 0.5) * MENU_ROW;
            let left = anchor.rect.left().clamp(bounds.left() + 8.0, (bounds.right() - width - 8.0).max(bounds.left() + 8.0));
            let top = top.clamp(bounds.top() + 8.0, (bounds.bottom() - height - 8.0).max(bounds.top() + 8.0));
            popup = popup
                .at_position(pos2(left, top))
                .align(RectAlign { parent: Align2::LEFT_TOP, child: Align2::LEFT_TOP })
                .without_alignment_fallback();
        } else {
            let align = match position {
                SelectPosition::Overlay | SelectPosition::BottomLeft => RectAlign::BOTTOM_START,
                SelectPosition::BottomCenter => RectAlign::BOTTOM,
                SelectPosition::BottomRight => RectAlign::BOTTOM_END,
                SelectPosition::TopLeft => RectAlign::TOP_START,
                SelectPosition::TopCenter => RectAlign::TOP,
                SelectPosition::TopRight => RectAlign::TOP_END,
            };
            popup = popup.align(align);
        }
        popup
    }

    fn paint_trigger(&self, ui: &Ui, rect: Rect, response: &Response, current: &str, t: Tokens, opened: bool) {
        let active = response.hovered() || opened || response.has_focus();
        match self.variant {
            SelectVariant::Default => {
                ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
                if active {
                    let border = if response.has_focus() { t.focus } else { t.field_hover_border };
                    ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, border), StrokeKind::Inside);
                }
            }
            SelectVariant::Ghost => {
                if active {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
                }
                if response.has_focus() {
                    ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
                }
            }
            SelectVariant::Outlined => {
                ui.painter().rect_filled(rect, CornerRadius::same(5), t.panel);
                let border = if response.has_focus() {
                    t.focus
                } else if active {
                    t.field_hover_border
                } else {
                    t.border
                };
                ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, border), StrokeKind::Inside);
            }
        }
        let mut label_x = rect.left() + 8.0;
        match self.lead {
            Some(SelectLead::Icon(icon)) => {
                icons::paint(ui.painter(), Rect::from_center_size(pos2(label_x + 6.0, rect.center().y), vec2(14.0, 14.0)), icon, t.icon_secondary);
                label_x += 18.0;
            }
            Some(SelectLead::Dot(color)) => {
                ui.painter().circle_filled(pos2(label_x + 5.0, rect.center().y), 4.5, color);
                label_x += 16.0;
            }
            None => {}
        }
        let text = widgets::elide(ui, current, (rect.right() - label_x - 20.0).max(0.0));
        let color = if self.enabled { t.text } else { t.text_secondary };
        let galley = ui.painter().layout_no_wrap(text, regular(11.0), color);
        ui.painter().galley(pos2(label_x, rect.center().y - galley.size().y / 2.0), galley, color);
        icons::paint_compact_chevron(
            ui.painter(),
            Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(CONTROL_CHEVRON_SIZE, CONTROL_CHEVRON_SIZE)),
            if self.enabled { t.icon_secondary } else { t.text_tertiary },
        );
    }
}
