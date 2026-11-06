//! Shared tooltip content and surface for controls in the editor and Workbench UI.

use egui::{Color32, Frame, InnerResponse, Margin, Pos2, Rect, Response, RichText, Shape, Stroke, Ui, WidgetText, pos2};

use super::Shortcut;
use crate::theme::{Tokens, regular};

pub struct Tooltip<'a> {
    title: &'a str,
    shortcut: Option<&'a str>,
    description: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct TooltipColors {
    surface: Color32,
    border: Color32,
    text: Color32,
    secondary: Color32,
}

impl TooltipColors {
    fn for_theme(t: Tokens) -> Self {
        let inverse = if t.dark { Tokens::light() } else { Tokens::dark() };
        Self { surface: inverse.panel, border: inverse.border, text: inverse.text, secondary: inverse.text_secondary }
    }
}

impl<'a> Tooltip<'a> {
    pub fn new(title: &'a str) -> Self {
        Self { title, shortcut: None, description: None }
    }

    pub fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn description(mut self, description: &'a str) -> Self {
        self.description = Some(description);
        self
    }

    fn from_text(text: &'a str) -> Self {
        if let Some((title, description)) = text.split_once('\n') {
            return Self::new(title).description(description);
        }
        if let Some((title, shortcut)) = text.rsplit_once("  hold ")
            && looks_like_shortcut(shortcut)
        {
            return Self::new(title).shortcut(shortcut);
        }
        if let Some((title, shortcut)) = text.rsplit_once("  ")
            && looks_like_shortcut(shortcut)
        {
            return Self::new(title).shortcut(shortcut);
        }
        if matches!(text, "Esc" | "Escape" | "Enter" | "Tab") {
            return Self::new("").shortcut(text);
        }
        Self::new(text)
    }

    fn frame(t: Tokens) -> Frame {
        let colors = TooltipColors::for_theme(t);
        Frame::NONE
            .fill(colors.surface)
            .stroke(Stroke::new(1.0, colors.border))
            .corner_radius(8)
            .inner_margin(Margin::symmetric(5, 2))
            .shadow(egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: t.shadow })
    }

    fn content(&self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let colors = TooltipColors::for_theme(t);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            if !self.title.is_empty() {
                ui.label(RichText::new(self.title).font(regular(11.0)).color(colors.text));
            }
            if let Some(shortcut) = self.shortcut {
                if !self.title.is_empty() {
                    ui.add_space(8.0);
                }
                let badge_background = if t.dark { Color32::from_black_alpha(13) } else { Color32::from_white_alpha(26) };
                Shortcut::new(shortcut).ink(colors.secondary).background(badge_background).show(ui);
            }
        });
        if let Some(description) = self.description {
            ui.label(RichText::new(description).font(regular(10.0)).color(colors.secondary));
        }
    }

    pub fn show_for(self, response: &Response) {
        self.show(response, false, false);
    }

    fn show_at_pointer(self, response: &Response) {
        self.show(response, true, false);
    }

    fn show_for_disabled(self, response: &Response) {
        self.show(response, false, true);
    }

    fn show(self, response: &Response, at_pointer: bool, disabled: bool) {
        let t = Tokens::get(&response.ctx);
        let frame = Self::frame(t);
        let mut tooltip = if disabled { egui::Tooltip::for_disabled(response) } else { egui::Tooltip::for_enabled(response) };
        let anchor = if at_pointer {
            tooltip = tooltip.at_pointer().gap(12.0);
            response.ctx.pointer_hover_pos().map(Rect::from_pos)
        } else {
            tooltip = tooltip.gap(8.0);
            let rect = response.interact_rect;
            Some(response.ctx.layer_transform_to_global(response.layer_id).map_or(rect, |transform| transform * rect))
        };
        if let Some(shown) = tooltip.popup.frame(frame).show(|ui| self.content(ui))
            && let Some(anchor) = anchor
        {
            let colors = TooltipColors::for_theme(t);
            paint_pointer(&shown.response, anchor, colors);
        }
    }

    /// An always visible specimen for tuning the same content and surface in Workbench UI.
    pub fn preview(self, ui: &mut Ui) -> InnerResponse<()> {
        let frame = Self::frame(Tokens::get(ui.ctx()));
        frame.show(ui, |ui| self.content(ui))
    }
}

fn looks_like_shortcut(text: &str) -> bool {
    !text.is_empty()
        && !text.contains(' ')
        && (text.chars().count() == 1
            || text.chars().any(|c| "⌃⌥⇧⌘⌫↵⇥⇞⇟←→↑↓".contains(c))
            || ["Ctrl", "Alt", "Shift", "Esc", "Enter", "Tab", "F1"].iter().any(|prefix| text.starts_with(prefix)))
}

/// A small filled triangle on the popup edge nearest its trigger.
fn paint_pointer(popup: &Response, anchor: Rect, colors: TooltipColors) {
    let rect = popup.rect;
    let clamp = |value: f32, start: f32, end: f32| value.clamp(start.min(end), start.max(end));
    let horizontal = clamp(anchor.center().x, rect.left() + 13.0, rect.right() - 13.0);
    let vertical = clamp(anchor.center().y, rect.top() + 13.0, rect.bottom() - 13.0);
    let points: [Pos2; 3] = if rect.max.y <= anchor.min.y {
        [pos2(horizontal - 5.0, rect.bottom() - 1.0), pos2(horizontal + 5.0, rect.bottom() - 1.0), pos2(horizontal, rect.bottom() + 7.0)]
    } else if rect.min.y >= anchor.max.y {
        [pos2(horizontal - 5.0, rect.top() + 1.0), pos2(horizontal + 5.0, rect.top() + 1.0), pos2(horizontal, rect.top() - 7.0)]
    } else if rect.max.x <= anchor.min.x {
        [pos2(rect.right() - 1.0, vertical - 5.0), pos2(rect.right() - 1.0, vertical + 5.0), pos2(rect.right() + 7.0, vertical)]
    } else {
        [pos2(rect.left() + 1.0, vertical - 5.0), pos2(rect.left() + 1.0, vertical + 5.0), pos2(rect.left() - 7.0, vertical)]
    };
    let painter = popup.ctx.layer_painter(popup.layer_id);
    painter.add(Shape::convex_polygon(points.to_vec(), colors.surface, Stroke::NONE));
    painter.add(Shape::line(vec![points[0], points[2], points[1]], Stroke::new(1.0, colors.border)));
}

/// The app's response tooltips all render through the Workbench Tooltip primitive.
pub trait TooltipResponseExt {
    fn lizard_tooltip(self, text: impl Into<WidgetText>) -> Self;
    fn lizard_tooltip_at_pointer(self, text: impl Into<WidgetText>) -> Self;
    fn lizard_disabled_tooltip(self, text: impl Into<WidgetText>) -> Self;
}

impl TooltipResponseExt for Response {
    fn lizard_tooltip(self, text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        Tooltip::from_text(text.text()).show_for(&self);
        self
    }

    fn lizard_tooltip_at_pointer(self, text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        Tooltip::from_text(text.text()).show_at_pointer(&self);
        self
    }

    fn lizard_disabled_tooltip(self, text: impl Into<WidgetText>) -> Self {
        let text = text.into();
        Tooltip::from_text(text.text()).show_for_disabled(&self);
        self
    }
}
