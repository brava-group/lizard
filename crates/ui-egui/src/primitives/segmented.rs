//! A selection group with one animated surface shared by its segments.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::ControlWidth;
use crate::a11y;
use crate::icons::{self, Icon, LayoutIcon, PositionIcon, VectorPointIcon};
use crate::theme::{Tokens, regular};
use crate::widgets;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SegmentSizing {
    #[default]
    Equal,
    Auto,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SegmentSize {
    #[default]
    Regular,
    Large,
}

#[derive(Clone, Copy, Debug)]
pub enum SegmentContent<'a> {
    Text(&'a str),
    Icon(Icon),
    LayoutIcon(LayoutIcon),
    PositionIcon(PositionIcon),
    VectorPointIcon(VectorPointIcon),
}

#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    pub label: &'a str,
    pub content: SegmentContent<'a>,
}

impl<'a> Segment<'a> {
    pub fn text(label: &'a str) -> Self {
        Self { label, content: SegmentContent::Text(label) }
    }

    pub fn icon(icon: Icon, label: &'a str) -> Self {
        Self { label, content: SegmentContent::Icon(icon) }
    }

    pub fn layout_icon(icon: LayoutIcon, label: &'a str) -> Self {
        Self { label, content: SegmentContent::LayoutIcon(icon) }
    }

    pub fn position_icon(icon: PositionIcon, label: &'a str) -> Self {
        Self { label, content: SegmentContent::PositionIcon(icon) }
    }

    pub fn vector_point_icon(icon: VectorPointIcon, label: &'a str) -> Self {
        Self { label, content: SegmentContent::VectorPointIcon(icon) }
    }
}

/// The caller owns the selected index. Animation is enabled unless explicitly disabled.
pub struct SegmentedControl<'a> {
    segments: &'a [Segment<'a>],
    selected: Option<usize>,
    width: ControlWidth,
    sizing: SegmentSizing,
    size: SegmentSize,
    icon_size: Option<f32>,
    animated: bool,
}

impl<'a> SegmentedControl<'a> {
    pub fn new(segments: &'a [Segment<'a>], selected: Option<usize>) -> Self {
        Self {
            segments,
            selected,
            width: ControlWidth::Hug,
            sizing: SegmentSizing::Equal,
            size: SegmentSize::Regular,
            icon_size: None,
            animated: true,
        }
    }

    pub fn width(mut self, width: ControlWidth) -> Self {
        self.width = width;
        self
    }

    pub fn sizing(mut self, sizing: SegmentSizing) -> Self {
        self.sizing = sizing;
        self
    }

    pub fn size(mut self, size: SegmentSize) -> Self {
        self.size = size;
        self
    }

    /// Override the artwork size without changing the segment hit targets.
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = Some(size);
        self
    }

    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }

    /// Returns a new selected index from a pointer or keyboard action.
    pub fn show(self, ui: &mut Ui) -> Option<usize> {
        if self.segments.is_empty() {
            return None;
        }
        let t = Tokens::get(ui.ctx());
        let height = if self.size == SegmentSize::Large { 32.0 } else { 24.0 };
        let natural: Vec<f32> = self
            .segments
            .iter()
            .map(|segment| match segment.content {
                SegmentContent::Icon(_) | SegmentContent::LayoutIcon(_) | SegmentContent::PositionIcon(_) | SegmentContent::VectorPointIcon(_) => {
                    height
                }
                SegmentContent::Text(label) => {
                    let text_width = ui.painter().layout_no_wrap(label.to_owned(), regular(11.0), t.text).size().x;
                    (text_width + 16.0).max(height)
                }
            })
            .collect();
        let natural_total = match self.sizing {
            SegmentSizing::Equal => natural.iter().copied().fold(0.0, f32::max) * natural.len() as f32,
            SegmentSizing::Auto => natural.iter().sum(),
        };
        let width = self.width.resolve(ui, natural_total + 2.0);
        let group_id = ui.next_auto_id().with("segmented");
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);

        let usable = (width - 2.0).max(0.0);
        let weights: Vec<f32> = match self.sizing {
            SegmentSizing::Equal => vec![1.0; natural.len()],
            SegmentSizing::Auto => natural,
        };
        let weight_sum: f32 = weights.iter().sum();
        let mut edges = Vec::with_capacity(weights.len() + 1);
        edges.push(rect.min.x + 1.0);
        let mut consumed = 0.0;
        for weight in &weights {
            consumed += *weight;
            edges.push(rect.min.x + 1.0 + usable * consumed / weight_sum.max(1.0));
        }
        let cell = |index: usize| Rect::from_min_max(pos2(edges[index], rect.min.y + 1.0), pos2(edges[index + 1], rect.max.y - 1.0));
        let mut responses = Vec::with_capacity(self.segments.len());
        for (index, segment) in self.segments.iter().enumerate() {
            let response = ui.interact(cell(index), group_id.with(index), Sense::click());
            let response = if matches!(
                segment.content,
                SegmentContent::Icon(_) | SegmentContent::LayoutIcon(_) | SegmentContent::PositionIcon(_) | SegmentContent::VectorPointIcon(_)
            ) {
                response.lizard_tooltip(segment.label)
            } else {
                response
            };
            a11y::radio(&response, segment.label, self.selected == Some(index));
            responses.push(response);
        }

        for (index, response) in responses.iter().enumerate() {
            if response.hovered() && self.selected != Some(index) {
                ui.painter().rect_filled(cell(index), CornerRadius::same(4), t.hover);
            }
        }
        if let Some(selected) = self.selected.filter(|index| *index < self.segments.len()) {
            let duration = if self.animated && ui.style().animation_time > 0.0 { 0.15 } else { 0.0 };
            let position = ui.ctx().animate_value_with_time(group_id.with("indicator"), selected as f32, duration);
            let first = position.floor().max(0.0) as usize;
            let first = first.min(self.segments.len() - 1);
            let last = (first + 1).min(self.segments.len() - 1);
            let amount = (position - first as f32).clamp(0.0, 1.0);
            let from = cell(first);
            let to = cell(last);
            let surface = Rect::from_min_max(
                pos2(egui::lerp(from.min.x..=to.min.x, amount), from.min.y),
                pos2(egui::lerp(from.max.x..=to.max.x, amount), from.max.y),
            );
            ui.painter().rect_filled(surface, CornerRadius::same(4), t.bg);
            ui.painter().rect_stroke(surface, CornerRadius::same(4), Stroke::new(1.0, t.border), StrokeKind::Inside);
        }

        for (index, segment) in self.segments.iter().enumerate() {
            let bounds = cell(index);
            let selected = self.selected == Some(index);
            let color = if selected { t.text } else { t.text_secondary };
            match segment.content {
                SegmentContent::Text(label) => {
                    let shown = widgets::elide_font(ui, label, (bounds.width() - 12.0).max(0.0), regular(11.0));
                    ui.painter().with_clip_rect(bounds).text(bounds.center(), Align2::CENTER_CENTER, shown, regular(11.0), color);
                }
                SegmentContent::Icon(icon) => {
                    let size = if matches!(
                        icon,
                        Icon::TypeAlignLeft
                            | Icon::TypeAlignCenter
                            | Icon::TypeAlignRight
                            | Icon::TypeAlignTop
                            | Icon::TypeAlignMiddle
                            | Icon::TypeAlignBottom
                            | Icon::TypeDialogJustify
                            | Icon::TypeDialogNone
                            | Icon::TypeDialogUnderline
                            | Icon::TypeDialogStrike
                            | Icon::TypeDialogUpper
                            | Icon::TypeDialogLower
                            | Icon::TypeDialogTitle
                            | Icon::TypeDialogSmallCaps
                            | Icon::TypeDialogTrimStandard
                            | Icon::TypeDialogTrimCap
                            | Icon::TypeDialogListBullets
                            | Icon::TypeDialogListNumbered
                            | Icon::TypeDialogTruncate
                    ) {
                        24.0
                    } else {
                        16.0
                    };
                    let size = self.icon_size.unwrap_or(size);
                    icons::paint(ui.painter(), Rect::from_center_size(bounds.center(), vec2(size, size)), icon, color);
                }
                SegmentContent::LayoutIcon(icon) => {
                    let size = self.icon_size.unwrap_or(24.0);
                    icons::paint_layout(ui.painter(), Rect::from_center_size(bounds.center(), vec2(size, size)), icon, color);
                }
                SegmentContent::PositionIcon(icon) => {
                    let size = self.icon_size.unwrap_or(16.0);
                    icons::paint_position(ui.painter(), Rect::from_center_size(bounds.center(), vec2(size, size)), icon, color, t.text_tertiary);
                }
                SegmentContent::VectorPointIcon(icon) => {
                    let size = self.icon_size.unwrap_or(20.0);
                    icons::paint_vector_point(ui.painter(), Rect::from_center_size(bounds.center(), vec2(size, size)), icon, color);
                }
            }
        }

        let clicked = responses.iter().position(egui::Response::clicked);
        let focused = responses.iter().position(egui::Response::has_focus);
        let keyboard = focused.and_then(|current| {
            ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowRight) || input.key_pressed(egui::Key::ArrowDown) {
                    Some((current + 1) % responses.len())
                } else if input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowUp) {
                    Some((current + responses.len() - 1) % responses.len())
                } else if input.key_pressed(egui::Key::Home) {
                    Some(0)
                } else if input.key_pressed(egui::Key::End) {
                    Some(responses.len() - 1)
                } else {
                    None
                }
            })
        });
        if let Some(index) = keyboard
            && let Some(response) = responses.get(index)
        {
            response.request_focus();
        }
        clicked.or(keyboard)
    }
}
