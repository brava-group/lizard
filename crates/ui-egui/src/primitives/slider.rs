//! Slider field shared by the catalog and the editor.

use egui::{Align, Align2, Color32, CornerRadius, Id, Key, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::ControlWidth;
use crate::a11y;
use crate::theme::{ColorRole, Tokens, regular};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SliderKind {
    #[default]
    Range,
    Hue,
    Opacity,
    Delta,
    Stepper,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SliderVariant {
    #[default]
    Default,
    Classic,
}

pub struct Slider<'a> {
    id: Id,
    label: &'a str,
    kind: SliderKind,
    variant: SliderVariant,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    value: Option<Option<f64>>,
    reference: Option<f64>,
    ticks: &'a [f64],
    text: bool,
    placeholder: &'a str,
    units: &'a str,
    transform: f64,
    color: Color32,
    precision: Option<usize>,
    disabled: bool,
    width: ControlWidth,
}

pub struct SliderResponse {
    pub response: Response,
    /// Raw value and whether the gesture has finished.
    pub change: Option<(f64, bool)>,
}

impl<'a> Slider<'a> {
    pub fn new(id: Id, label: &'a str) -> Self {
        Self {
            id,
            label,
            kind: SliderKind::Range,
            variant: SliderVariant::Default,
            min: None,
            max: None,
            step: None,
            value: None,
            reference: None,
            ticks: &[],
            text: true,
            placeholder: "##",
            units: "",
            transform: 1.0,
            color: Color32::from_rgb(0xff, 0, 0),
            precision: None,
            disabled: false,
            width: ControlWidth::Hug,
        }
    }

    pub fn kind(mut self, kind: SliderKind) -> Self {
        self.kind = kind;
        self
    }
    pub fn variant(mut self, variant: SliderVariant) -> Self {
        self.variant = variant;
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }
    /// `None` represents a mixed selection. Without this call, the kind's default is used.
    pub fn value(mut self, value: Option<f64>) -> Self {
        self.value = Some(value);
        self
    }
    /// Draw a reference tick and snap to it when the pointer approaches it.
    pub fn reference(mut self, value: f64) -> Self {
        self.reference = Some(value);
        self
    }
    pub fn ticks(mut self, ticks: &'a [f64]) -> Self {
        self.ticks = ticks;
        self
    }
    pub fn text(mut self, text: bool) -> Self {
        self.text = text;
        self
    }
    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }
    pub fn units(mut self, units: &'a str) -> Self {
        self.units = units;
        self
    }
    pub fn transform(mut self, multiplier: f64) -> Self {
        self.transform = multiplier;
        self
    }
    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }
    pub fn precision(mut self, digits: usize) -> Self {
        self.precision = Some(digits.min(8));
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn width(mut self, width: ControlWidth) -> Self {
        self.width = width;
        self
    }

    pub fn show(self, ui: &mut Ui) -> SliderResponse {
        let t = Tokens::get(ui.ctx());
        let (default_min, default_max, default_step): (f64, f64, f64) = match self.kind {
            SliderKind::Range => (0.0, 100.0, 1.0),
            SliderKind::Hue => (0.0, 255.0, 1.0),
            SliderKind::Opacity => (0.0, 100.0, 0.1),
            SliderKind::Delta => (-100.0, 100.0, 1.0),
            SliderKind::Stepper => (0.0, 100.0, 25.0),
        };
        let min = self.min.filter(|v| v.is_finite()).unwrap_or(default_min);
        let max = self.max.filter(|v| v.is_finite() && *v > min).unwrap_or(default_max.max(min + 1.0));
        let step = self.step.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default_step);
        let transform = if self.transform.is_finite() && self.transform != 0.0 { self.transform } else { 1.0 };
        let value = self
            .value
            .unwrap_or_else(|| {
                Some(if self.kind == SliderKind::Range {
                    (min + max) / 2.0
                } else if self.kind == SliderKind::Delta {
                    self.reference.unwrap_or(0.0)
                } else {
                    min
                })
            })
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(min, max));
        let reference = self.reference.or((self.kind == SliderKind::Delta).then_some(0.0)).filter(|v| v.is_finite() && *v >= min && *v <= max);
        let modern = self.variant == SliderVariant::Default;
        let width = self.width.resolve(ui, if self.text { 224.0 } else { 176.0 });
        let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
        let text_width = if self.text { (if modern { 56.0_f32 } else { 48.0 }).min((width * 0.42).max(0.0)) } else { 0.0 };
        let text_gap = if self.text && !modern { 8.0 } else { 0.0 };
        let slider_bounds = Rect::from_min_max(rect.min, pos2(rect.max.x - text_width - text_gap, rect.max.y));
        let track_height = if modern { 6.0 } else { 16.0 };
        let track = Rect::from_center_size(
            pos2((slider_bounds.min.x + slider_bounds.max.x) / 2.0, rect.center().y),
            vec2((slider_bounds.width() - if modern { 20.0 } else { 0.0 }).max(1.0), track_height),
        );
        let response = ui.interact(slider_bounds, self.id, if self.disabled { Sense::hover() } else { Sense::click_and_drag() });
        a11y::slider(&response, self.label, value, (min, max));
        let painter = ui.painter().clone();
        let dim = if self.disabled { 0.48 } else { 1.0 };
        if modern {
            painter.rect_filled(rect, CornerRadius::same(5), t.field.gamma_multiply(dim));
            if !self.disabled && (response.hovered() || response.has_focus()) {
                painter.rect_stroke(
                    rect,
                    CornerRadius::same(5),
                    Stroke::new(1.0, if response.has_focus() { t.focus } else { t.field_hover_border }),
                    StrokeKind::Inside,
                );
            }
        }
        let span = max - min;
        let value_left = track.min.x + if modern { 0.0 } else { 8.0 };
        let value_width = (track.width() - if modern { 0.0 } else { 16.0 }).max(1.0);
        let x_of = |v: f64| value_left + (((v - min) / span).clamp(0.0, 1.0) as f32) * value_width;
        let track_round = CornerRadius::same(if modern { 3 } else { 8 });
        painter.rect_filled(track, track_round, if modern { ColorRole::BgTertiary.value(t.dark) } else { t.panel }.gamma_multiply(dim));
        if !modern {
            painter.rect_stroke(track, track_round, Stroke::new(1.0, t.border.gamma_multiply(dim)), StrokeKind::Inside);
        }
        match self.kind {
            SliderKind::Hue => paint_hue(ui, track, dim),
            SliderKind::Opacity => paint_opacity(ui, track, self.color, dim),
            SliderKind::Range | SliderKind::Delta | SliderKind::Stepper => {
                if let Some(value) = value {
                    let from_reference = self.kind == SliderKind::Delta || (self.kind == SliderKind::Stepper && reference.is_some());
                    let start = if from_reference { reference.unwrap_or(min) } else { min };
                    let start_x = if from_reference { x_of(start) } else { track.min.x };
                    let value_x = x_of(value);
                    let left = if from_reference && value >= start { start_x - track_height / 2.0 } else { start_x.min(value_x) };
                    let right = if from_reference && value < start { start_x + track_height / 2.0 } else { value_x };
                    if right > left && (!from_reference || (value - start).abs() > 1e-9) {
                        painter.rect_filled(
                            Rect::from_min_max(pos2(left, track.min.y), pos2(right, track.max.y)),
                            track_round,
                            t.accent.gamma_multiply(dim),
                        );
                    }
                }
            }
        }
        let generated_ticks: Vec<f64> = if !self.ticks.is_empty() {
            self.ticks.to_vec()
        } else if self.kind == SliderKind::Stepper {
            let count = ((span / step).floor() as usize).min(100);
            (0..=count).map(|i| min + i as f64 * step).collect()
        } else {
            Vec::new()
        };
        for tick in generated_ticks.iter().copied().chain(reference) {
            if tick >= min && tick <= max {
                let is_reference = reference.is_some_and(|v| (v - tick).abs() < 1e-9);
                painter.circle_filled(
                    pos2(x_of(tick), track.center().y),
                    if is_reference { 2.0 } else { 1.5 },
                    if is_reference { t.text.gamma_multiply(dim) } else { t.text_tertiary.gamma_multiply(dim) },
                );
            }
        }
        if let Some(value) = value {
            let center = pos2(x_of(value), track.center().y);
            let radius = if modern { 6.0 } else { 8.0 };
            let white = Color32::WHITE.gamma_multiply(dim);
            painter.circle_stroke(center + vec2(0.0, 0.5), radius + 0.5, Stroke::new(1.0, Color32::BLACK.gamma_multiply(0.18 * dim)));
            if !modern && (value - reference.unwrap_or(min)).abs() > 1e-9 {
                // The classic handle keeps a blue centre after it leaves the reference point.
                painter.circle_filled(center, radius - 3.0, t.accent.gamma_multiply(dim));
                // egui expands the stroke outward from this radius. Keep its outer edge
                // at the same eight-point radius as the solid center-state thumb.
                painter.circle_stroke(center, radius - 4.0, Stroke::new(4.0, white));
            } else if modern && matches!(self.kind, SliderKind::Hue | SliderKind::Opacity) {
                // The colour track stays visible through the centre of the modern handle.
                painter.circle_stroke(center, radius - 3.0, Stroke::new(3.0, white));
            } else {
                painter.circle_filled(center, radius, white);
            }
            if response.has_focus() {
                painter.circle_stroke(center, radius + 2.0, Stroke::new(1.0, t.focus));
            }
        }
        let mut change = None;
        if !self.disabled {
            if (response.dragged() || response.clicked() || response.drag_started())
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let raw = min + f64::from(((pointer.x - value_left) / value_width).clamp(0.0, 1.0)) * span;
                let snapped = ((raw - min) / step).round() * step + min;
                let snapped = reference.filter(|r| (raw - r).abs() <= span * 0.012).unwrap_or(snapped).clamp(min, max);
                if value.is_none_or(|old| (old - snapped).abs() > 1e-9) || response.drag_stopped() || response.clicked() {
                    change = Some((snapped, response.drag_stopped() || response.clicked()));
                }
            } else if response.drag_stopped() {
                change = value.map(|v| (v, true));
            }
            if response.has_focus() {
                let direction = ui.input(|i| {
                    let factor = if i.modifiers.shift { 10.0 } else { 1.0 };
                    if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowUp) {
                        Some(step * factor)
                    } else if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowDown) {
                        Some(-step * factor)
                    } else if i.key_pressed(Key::Home) {
                        Some(min - value.unwrap_or(min))
                    } else if i.key_pressed(Key::End) {
                        Some(max - value.unwrap_or(min))
                    } else {
                        None
                    }
                });
                if let Some(by) = direction {
                    change = Some(((value.unwrap_or(min) + by).clamp(min, max), true));
                }
            }
        }
        if self.text {
            let field = Rect::from_min_max(pos2(slider_bounds.max.x + text_gap, rect.min.y), rect.max);
            if modern {
                painter.line_segment([pos2(field.min.x, field.min.y + 2.0), pos2(field.min.x, field.max.y - 2.0)], Stroke::new(1.0, t.bg));
            } else {
                painter.rect_filled(field, CornerRadius::same(5), t.field.gamma_multiply(dim));
            }
            let display = value.map(|v| format_value(v * transform, self.precision)).unwrap_or_else(|| self.placeholder.to_owned());
            let edit_id = self.id.with("number");
            let buffer_id = self.id.with("buffer");
            let mut buffer = ui.data(|data| data.get_temp::<String>(buffer_id)).unwrap_or_else(|| display.clone());
            if !ui.memory(|m| m.has_focus(edit_id)) {
                buffer = display.clone();
            }
            let unit_width = if self.units.is_empty() {
                0.0
            } else {
                ui.painter().layout_no_wrap(self.units.to_owned(), regular(11.0), t.text_secondary).size().x + 5.0
            };
            let edit_rect =
                Rect::from_min_max(pos2(field.min.x + 7.0, field.min.y), pos2((field.max.x - unit_width - 2.0).max(field.min.x + 8.0), field.max.y));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect));
            let edit = child.add_enabled(
                !self.disabled,
                egui::TextEdit::singleline(&mut buffer)
                    .id(edit_id)
                    .frame(egui::Frame::NONE)
                    .font(regular(11.0))
                    .text_color(if value.is_none() && !ui.memory(|m| m.has_focus(edit_id)) {
                        t.text_tertiary.gamma_multiply(dim)
                    } else {
                        t.text.gamma_multiply(dim)
                    })
                    .desired_width(edit_rect.width())
                    .vertical_align(Align::Center)
                    .min_size(vec2(0.0, field.height())),
            );
            if !self.units.is_empty() {
                painter.text(
                    pos2(field.max.x - 5.0, field.center().y),
                    Align2::RIGHT_CENTER,
                    self.units,
                    regular(11.0),
                    t.text_secondary.gamma_multiply(dim),
                );
            }
            let committed = edit.lost_focus() || (edit.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
            if committed && !self.disabled {
                let raw = buffer.trim().trim_end_matches(self.units).trim();
                if let Some(parsed) = crate::field_math::eval(raw, value.map(|v| v * transform)).filter(|v| v.is_finite()) {
                    change = Some(((parsed / transform).clamp(min, max), true));
                }
                buffer = display;
            }
            ui.data_mut(|data| data.insert_temp(buffer_id, buffer));
        }
        SliderResponse { response, change }
    }
}

fn format_value(value: f64, precision: Option<usize>) -> String {
    if let Some(digits) = precision {
        return format!("{value:.digits$}");
    }
    let mut text = format!("{value:.6}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn hue_color(fraction: f64) -> Color32 {
    let h = (fraction.rem_euclid(1.0) * 6.0) as f32;
    let x = (1.0 - (h.rem_euclid(2.0) - 1.0).abs()) * 255.0;
    let (r, g, b) = match h as u8 {
        0 => (255.0, x, 0.0),
        1 => (x, 255.0, 0.0),
        2 => (0.0, 255.0, x),
        3 => (0.0, x, 255.0),
        4 => (x, 0.0, 255.0),
        _ => (255.0, 0.0, x),
    };
    Color32::from_rgb(r as u8, g as u8, b as u8)
}

fn paint_hue(ui: &Ui, rect: Rect, dim: f32) {
    let painter = ui.painter().with_clip_rect(rect);
    for index in 0..48 {
        let x0 = rect.min.x + rect.width() * index as f32 / 48.0;
        let x1 = rect.min.x + rect.width() * (index + 1) as f32 / 48.0;
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, rect.min.y), pos2(x1 + 0.5, rect.max.y)),
            CornerRadius::ZERO,
            hue_color(index as f64 / 48.0).gamma_multiply(dim),
        );
    }
}

fn paint_opacity(ui: &Ui, rect: Rect, color: Color32, dim: f32) {
    let painter = ui.painter().with_clip_rect(rect);
    let cell = 4.0;
    for row in 0..((rect.height() / cell).ceil() as usize) {
        for col in 0..((rect.width() / cell).ceil() as usize) {
            let shade = if (row + col) % 2 == 0 { 210 } else { 255 };
            let block = Rect::from_min_size(pos2(rect.min.x + col as f32 * cell, rect.min.y + row as f32 * cell), vec2(cell, cell));
            painter.rect_filled(block.intersect(rect), CornerRadius::ZERO, Color32::from_gray(shade).gamma_multiply(dim));
        }
    }
    for index in 0..48 {
        let x0 = rect.min.x + rect.width() * index as f32 / 48.0;
        let x1 = rect.min.x + rect.width() * (index + 1) as f32 / 48.0;
        let alpha = (255.0 * (index as f32 + 0.5) / 48.0) as u8;
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, rect.min.y), pos2(x1 + 0.5, rect.max.y)),
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha).gamma_multiply(dim),
        );
    }
}
