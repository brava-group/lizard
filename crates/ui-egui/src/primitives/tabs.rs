//! Compact content tabs with roving keyboard selection and horizontal overflow.

use egui::{Align2, Color32, CornerRadius, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::ControlWidth;
use crate::a11y;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, regular, semibold};

#[derive(Clone, Copy, Debug)]
pub struct Tab<'a> {
    pub label: &'a str,
    pub value: &'a str,
    pub disabled: bool,
}

impl<'a> Tab<'a> {
    pub fn new(label: &'a str) -> Self {
        Self { label, value: label, disabled: false }
    }

    pub fn value(mut self, value: &'a str) -> Self {
        self.value = value;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabOverflow {
    #[default]
    Buttons,
    Fade,
}

/// The caller owns the selection and renders the chosen tab's content.
pub struct Tabs<'a> {
    id: Id,
    tabs: &'a [Tab<'a>],
    selected: Option<usize>,
    selected_fill: Option<Color32>,
    hover_fill: Option<Color32>,
    text_colors: Option<(Color32, Color32)>,
    width: ControlWidth,
    overflow: TabOverflow,
    disabled: bool,
    allow_empty: bool,
}

impl<'a> Tabs<'a> {
    pub fn new(id: Id, tabs: &'a [Tab<'a>], selected: Option<usize>) -> Self {
        Self {
            id,
            tabs,
            selected,
            selected_fill: None,
            hover_fill: None,
            text_colors: None,
            width: ControlWidth::Fill,
            overflow: TabOverflow::Buttons,
            disabled: false,
            allow_empty: false,
        }
    }

    pub fn width(mut self, width: ControlWidth) -> Self {
        self.width = width;
        self
    }
    pub fn selected_fill(mut self, color: Color32) -> Self {
        self.selected_fill = Some(color);
        self
    }
    pub fn hover_fill(mut self, color: Color32) -> Self {
        self.hover_fill = Some(color);
        self
    }
    pub fn text_colors(mut self, selected: Color32, unselected: Color32) -> Self {
        self.text_colors = Some((selected, unselected));
        self
    }
    pub fn overflow(mut self, overflow: TabOverflow) -> Self {
        self.overflow = overflow;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn allow_empty(mut self, allow_empty: bool) -> Self {
        self.allow_empty = allow_empty;
        self
    }

    /// Return the newly selected index from a click or an arrow/Home/End key.
    pub fn show(self, ui: &mut Ui) -> Option<usize> {
        if self.tabs.is_empty() {
            return None;
        }
        let t = Tokens::get(ui.ctx());
        let widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|tab| {
                let regular_width = ui.painter().layout_no_wrap(tab.label.to_owned(), regular(11.0), t.text).size().x;
                let strong_width = ui.painter().layout_no_wrap(tab.label.to_owned(), semibold(11.0), t.text).size().x;
                regular_width.max(strong_width).ceil() + 16.0
            })
            .collect();
        let natural_width = widths.iter().sum::<f32>() + 4.0 * self.tabs.len().saturating_sub(1) as f32 + 12.0;
        let width = self.width.resolve(ui, natural_width);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
        let viewport_outer = Rect::from_min_max(pos2(rect.min.x + 4.0, rect.min.y + 4.0), pos2(rect.max.x - 8.0, rect.max.y - 4.0));
        let total = natural_width - 12.0;
        let overflowing = total > viewport_outer.width();
        let viewport = if overflowing && self.overflow == TabOverflow::Buttons {
            Rect::from_min_max(pos2(viewport_outer.min.x + 16.0, viewport_outer.min.y), pos2(viewport_outer.max.x - 16.0, viewport_outer.max.y))
        } else {
            viewport_outer
        };
        let max_scroll = (total - viewport.width()).max(0.0);
        let scroll_id = self.id.with("tab_scroll");
        let mut scroll = ui.data(|data| data.get_temp::<f32>(scroll_id)).unwrap_or(0.0).clamp(0.0, max_scroll);

        let active = self
            .selected
            .filter(|index| *index < self.tabs.len() && !self.tabs[*index].disabled)
            .or_else(|| (!self.allow_empty).then(|| self.tabs.iter().position(|tab| !tab.disabled)).flatten());
        let last_selected_id = self.id.with("last_selected");
        let last_selected = ui.data(|data| data.get_temp::<Option<usize>>(last_selected_id));
        if overflowing
            && last_selected != Some(active)
            && let Some(index) = active
        {
            let start = widths.iter().take(index).sum::<f32>() + 4.0 * index as f32;
            let center = start + widths[index] / 2.0;
            scroll = (center - viewport.width() / 2.0).clamp(0.0, max_scroll);
        }
        ui.data_mut(|data| data.insert_temp(last_selected_id, active));
        if overflowing && !self.disabled && viewport.contains(ui.input(|input| input.pointer.hover_pos()).unwrap_or_default()) {
            let delta = ui.input(|input| input.smooth_scroll_delta);
            if delta.x != 0.0 || delta.y != 0.0 {
                scroll = (scroll - if delta.x != 0.0 { delta.x } else { delta.y }).clamp(0.0, max_scroll);
            }
        }

        if overflowing && self.overflow == TabOverflow::Buttons {
            let left = Rect::from_min_max(viewport_outer.min, pos2(viewport.min.x, viewport_outer.max.y));
            let right = Rect::from_min_max(pos2(viewport.max.x, viewport_outer.min.y), viewport_outer.max);
            if tab_nav(ui, self.id.with("left"), left, Icon::ChevronLeft, scroll > 0.0, self.disabled, t) {
                scroll = (scroll - viewport.width() * 0.8).max(0.0);
            }
            if tab_nav(ui, self.id.with("right"), right, Icon::ChevronRight, scroll < max_scroll, self.disabled, t) {
                scroll = (scroll + viewport.width() * 0.8).min(max_scroll);
            }
        }
        ui.data_mut(|data| data.insert_temp(scroll_id, scroll));

        let painter = ui.painter().with_clip_rect(viewport);
        let mut x = viewport.min.x - scroll;
        let mut changed = None;
        let mut focused = None;
        for (index, (tab, tab_width)) in self.tabs.iter().zip(widths).enumerate() {
            let tab_rect = Rect::from_min_size(pos2(x, viewport.min.y), vec2(tab_width, 24.0));
            x += tab_width + 4.0;
            let visible = tab_rect.intersect(viewport);
            let enabled = !self.disabled && !tab.disabled;
            let response = if visible.width() > 0.0 {
                Some(ui.interact(visible, self.id.with((index, tab.value)), if enabled { Sense::click() } else { Sense::hover() }))
            } else {
                None
            };
            if let Some(response) = &response {
                a11y::tab(response, tab.label, active == Some(index));
                if response.clicked() && enabled && active != Some(index) {
                    changed = Some(index);
                }
                if response.has_focus() && enabled {
                    focused = Some(index);
                }
            }
            let selected = active == Some(index);
            let hover = response.as_ref().is_some_and(|response| response.hovered()) && enabled;
            if self.tabs.len() > 1 && (selected || hover) {
                let fill = if selected { self.selected_fill.unwrap_or(t.field) } else { self.hover_fill.unwrap_or(t.field) };
                painter.rect_filled(tab_rect, CornerRadius::same(5), fill.gamma_multiply(if enabled { 1.0 } else { 0.4 }));
            }
            let (selected_text, unselected_text) = self.text_colors.unwrap_or((t.text, t.text_secondary));
            let color = if selected { selected_text } else { unselected_text }.gamma_multiply(if enabled { 1.0 } else { 0.4 });
            let font = if selected { semibold(11.0) } else { regular(11.0) };
            painter.text(tab_rect.center(), Align2::CENTER_CENTER, tab.label, font, color);
            if response.as_ref().is_some_and(|response| response.has_focus()) {
                painter.rect_stroke(tab_rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Outside);
            }
        }

        if let Some(current) = focused {
            let key = ui.input(|input| {
                if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::ArrowDown) {
                    Some(1)
                } else if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::ArrowUp) {
                    Some(-1)
                } else if input.key_pressed(Key::Home) {
                    Some(2)
                } else if input.key_pressed(Key::End) {
                    Some(-2)
                } else {
                    None
                }
            });
            if let Some(key) = key {
                let available: Vec<usize> = self.tabs.iter().enumerate().filter_map(|(index, tab)| (!tab.disabled).then_some(index)).collect();
                if let Some(position) = available.iter().position(|index| *index == current) {
                    let next_position = match key {
                        1 => (position + 1) % available.len(),
                        -1 => (position + available.len() - 1) % available.len(),
                        2 => 0,
                        _ => available.len() - 1,
                    };
                    let next = available[next_position];
                    if next != current {
                        changed = Some(next);
                        ui.memory_mut(|memory| memory.request_focus(self.id.with((next, self.tabs[next].value))));
                    }
                }
            }
        }

        if overflowing && self.overflow == TabOverflow::Fade {
            let overlay = ui.painter();
            if scroll > 0.0 {
                paint_fade(overlay, viewport_outer, true, t.bg);
            }
            if scroll < max_scroll {
                paint_fade(overlay, viewport_outer, false, t.bg);
            }
        }
        changed
    }
}

fn tab_nav(ui: &mut Ui, id: Id, rect: Rect, icon: Icon, visible: bool, disabled: bool, t: Tokens) -> bool {
    if !visible {
        return false;
    }
    let response = ui.interact(rect, id, if disabled { Sense::hover() } else { Sense::click() });
    a11y::button(&response, if icon == Icon::ChevronLeft { "Scroll tabs left" } else { "Scroll tabs right" });
    ui.painter().rect_filled(rect, CornerRadius::same(5), if response.hovered() && !disabled { t.hover } else { t.bg });
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(14.0, 14.0)), icon, t.icon_secondary);
    response.clicked() && !disabled
}

fn paint_fade(painter: &egui::Painter, rect: Rect, left: bool, color: egui::Color32) {
    for step in 0..12 {
        let alpha = step as f32 / 12.0 * 0.85;
        let x = if left { rect.min.x + 12.0 - step as f32 } else { rect.max.x - 12.0 + step as f32 };
        painter.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(1.0, color.gamma_multiply(alpha)));
    }
}
