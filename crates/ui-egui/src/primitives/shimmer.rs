//! Animated highlight passing through text, based on FigUI3's shimmer text treatment.

use std::time::Duration;

use egui::{Color32, Response, Sense, Ui, pos2, vec2};

use super::ControlWidth;
use crate::theme::{Tokens, regular};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShimmerDirection {
    #[default]
    Horizontal,
    Vertical,
    Diagonal,
}

/// A non-interactive text label with a moving light band.
pub struct ShimmerText<'a> {
    text: &'a str,
    direction: ShimmerDirection,
    duration: f32,
    playing: bool,
    size: f32,
    width: ControlWidth,
    subtle_color: Option<Color32>,
    bright_color: Option<Color32>,
}

impl<'a> ShimmerText<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            direction: ShimmerDirection::Horizontal,
            duration: 1.5,
            playing: true,
            size: 13.0,
            width: ControlWidth::Hug,
            subtle_color: None,
            bright_color: None,
        }
    }

    pub fn direction(mut self, direction: ShimmerDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Seconds per sweep. Values at or below zero show the paused appearance.
    pub fn duration(mut self, seconds: f32) -> Self {
        self.duration = seconds;
        self
    }

    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    pub fn size(mut self, points: f32) -> Self {
        self.size = points;
        self
    }

    pub fn width(mut self, width: ControlWidth) -> Self {
        self.width = width;
        self
    }

    pub fn colors(mut self, subtle: Color32, bright: Color32) -> Self {
        self.subtle_color = Some(subtle);
        self.bright_color = Some(bright);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let tokens = Tokens::get(ui.ctx());
        let subtle = self.subtle_color.unwrap_or(tokens.text_tertiary);
        let bright = self.bright_color.unwrap_or(tokens.text);
        let font = regular(self.size.clamp(1.0, 256.0));
        let intrinsic = ui.painter().layout_no_wrap(self.text.to_owned(), font.clone(), subtle).size().x;
        let width = self.width.resolve(ui, intrinsic.max(1.0));
        let galley = ui.painter().layout(self.text.to_owned(), font, subtle, width);
        let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y.max(1.0)), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let origin = rect.min;
        let painter = ui.painter().with_clip_rect(rect);
        if !self.playing || !self.duration.is_finite() || self.duration <= 0.0 {
            painter.galley_with_override_text_color(origin, galley, self.subtle_color.unwrap_or(tokens.text_secondary));
            return response;
        }

        painter.galley(origin, galley.clone(), subtle);
        let time = ui.input(|input| input.time);
        let progress = (time.rem_euclid(f64::from(self.duration)) / f64::from(self.duration)) as f32;
        let center = -0.25 + progress * 1.5;
        let step = (2.0 / ui.ctx().pixels_per_point()).max(0.5);
        let row_step = if self.direction == ShimmerDirection::Diagonal { step * 2.0 } else { rect.height().max(step) };
        let column_step = if self.direction == ShimmerDirection::Vertical { rect.width().max(step) } else { step };

        let mut y = rect.min.y;
        while y < rect.max.y {
            let bottom = (y + row_step).min(rect.max.y);
            let ny = ((y + bottom) * 0.5 - rect.min.y) / rect.height().max(1.0);
            let mut x = rect.min.x;
            while x < rect.max.x {
                let right = (x + column_step).min(rect.max.x);
                let nx = ((x + right) * 0.5 - rect.min.x) / rect.width().max(1.0);
                let position = match self.direction {
                    ShimmerDirection::Horizontal => nx,
                    ShimmerDirection::Vertical => ny,
                    ShimmerDirection::Diagonal => (nx + ny) * 0.5,
                };
                let strength = (1.0 - ((position - center) / 0.25).abs()).max(0.0);
                if strength > 0.0 {
                    let color = blend(subtle, bright, strength);
                    let clip = egui::Rect::from_min_max(pos2(x, y), pos2(right, bottom));
                    painter.with_clip_rect(clip).galley_with_override_text_color(origin, galley.clone(), color);
                }
                x = right;
            }
            y = bottom;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(16));
        response
    }
}

fn blend(from: Color32, to: Color32, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color32::from_rgba_unmultiplied(mix(from.r(), to.r()), mix(from.g(), to.g()), mix(from.b(), to.b()), mix(from.a(), to.a()))
}
