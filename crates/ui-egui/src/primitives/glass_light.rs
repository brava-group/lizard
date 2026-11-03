//! Interactive preview for the direction and strength of a glass effect's light.

use egui::{Color32, CornerRadius, CursorIcon, Id, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::a11y;
use crate::i18n::tr;
use crate::theme::Tokens;

pub struct GlassLightControl {
    id: Id,
    angle: f64,
    intensity: f64,
}

pub struct GlassLightResponse {
    pub angle: Option<f64>,
    pub response: Response,
}

impl GlassLightControl {
    pub fn new(id: Id, angle: f64, intensity: f64) -> Self {
        Self { id, angle, intensity }
    }

    pub fn show(self, ui: &mut Ui, rect: Rect) -> GlassLightResponse {
        let response = ui.interact(rect, self.id, Sense::click_and_drag());
        a11y::slider(&response, tr("Light|glass"), Some(self.angle), (-180.0, 180.0));
        if response.hovered() || response.dragged() {
            ui.output_mut(|output| output.cursor_icon = if response.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }
        let picked = if response.clicked() || response.dragged() || response.drag_started() || response.drag_stopped() {
            response.interact_pointer_pos().and_then(|pointer| angle_from_pointer(rect.center(), pointer))
        } else {
            None
        };
        let angle = picked.filter(|angle| (angle - self.angle).abs() > 0.1);
        let shown_angle = angle.unwrap_or(self.angle);
        paint(ui, rect, shown_angle, self.intensity);
        GlassLightResponse { angle, response }
    }
}

fn angle_from_pointer(center: Pos2, pointer: Pos2) -> Option<f64> {
    let delta = pointer - center;
    (delta.length_sq() >= 16.0).then(|| f64::from(delta.y.atan2(-delta.x).to_degrees()))
}

fn source_direction(angle: f64) -> Vec2 {
    let radians = if angle.is_finite() { angle.to_radians() as f32 } else { 0.0 };
    vec2(-radians.cos(), radians.sin())
}

fn beam_alpha(rect: Rect, point: Pos2, toward_center: Vec2, intensity: f32) -> u8 {
    let delta = point - rect.center();
    let along = delta.dot(toward_center);
    let across = delta.x * toward_center.y - delta.y * toward_center.x;
    let spread = 7.0 + along.max(0.0) * 0.7;
    let lateral = (1.0 - (across.abs() / spread).clamp(0.0, 1.0)).powf(1.5);
    let forward = ((along + 5.0) / 11.0).clamp(0.0, 1.0);
    let reach = (1.0 - along.max(0.0) / (rect.width() * 1.4)).clamp(0.0, 1.0);
    let edge = ((point.x - rect.min.x).min(rect.max.x - point.x).min(point.y - rect.min.y).min(rect.max.y - point.y) / 3.0).clamp(0.0, 1.0);
    (255.0 * intensity * 0.62 * forward * lateral * reach * edge).round() as u8
}

fn paint(ui: &Ui, rect: Rect, angle: f64, intensity: f64) {
    let tokens = Tokens::get(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(7), tokens.field);

    let source = source_direction(angle);
    let toward_center = -source;
    let intensity = if intensity.is_finite() { intensity.clamp(0.0, 1.0) as f32 } else { 0.0 };
    let mut beam = Mesh::default();
    const CELLS: u32 = 16;
    for y in 0..=CELLS {
        for x in 0..=CELLS {
            let point = pos2(rect.min.x + rect.width() * x as f32 / CELLS as f32, rect.min.y + rect.height() * y as f32 / CELLS as f32);
            beam.colored_vertex(point, Color32::from_white_alpha(beam_alpha(rect, point, toward_center, intensity)));
        }
    }
    for y in 0..CELLS {
        for x in 0..CELLS {
            let a = y * (CELLS + 1) + x;
            let b = a + 1;
            let c = a + CELLS + 1;
            let d = c + 1;
            beam.add_triangle(a, b, c);
            beam.add_triangle(b, d, c);
        }
    }
    painter.add(Shape::mesh(beam));

    let anchor = Rect::from_center_size(rect.center(), vec2(7.0, 7.0));
    painter.rect_stroke(anchor, CornerRadius::same(2), Stroke::new(1.2, tokens.text.gamma_multiply(0.72)), StrokeKind::Inside);

    let light = rect.center() + source * rect.width() * 0.35;
    let forward = toward_center;
    let side = vec2(-forward.y, forward.x);
    let point = |x: f32, y: f32| light + forward * x + side * y;
    let color = tokens.accent;
    let stroke = Stroke::new(1.4, color);
    painter.add(Shape::closed_line(
        vec![point(-4.0, -3.0), point(1.5, -3.0), point(5.0, -1.5), point(5.0, 1.5), point(1.5, 3.0), point(-4.0, 3.0)],
        stroke,
    ));
    painter.line_segment([point(-5.0, -5.0), point(-5.0, 5.0)], stroke);
    painter.line_segment([point(-7.0, -2.5), point(-5.0, -2.5)], stroke);
    painter.line_segment([point(-7.0, 2.5), point(-5.0, 2.5)], stroke);
    painter.rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, tokens.border), StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_direction_matches_the_glass_light_angle() {
        let center = pos2(28.0, 28.0);
        assert!((angle_from_pointer(center, pos2(14.0, 14.0)).unwrap_or_default() + 45.0).abs() < 0.01);
        assert!((angle_from_pointer(center, pos2(42.0, 28.0)).unwrap_or_default().abs() - 180.0).abs() < 0.01);
        assert_eq!(angle_from_pointer(center, center), None);
        let northwest = source_direction(-45.0);
        assert!(northwest.x < -0.7 && northwest.y < -0.7);
    }

    #[test]
    fn beam_brightness_follows_intensity_and_direction() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(56.0, 56.0));
        let toward_center = -source_direction(-45.0);
        let on_beam = pos2(37.0, 37.0);
        let behind_light = pos2(19.0, 19.0);
        assert_eq!(beam_alpha(rect, on_beam, toward_center, 0.0), 0);
        assert!(beam_alpha(rect, on_beam, toward_center, 1.0) > beam_alpha(rect, on_beam, toward_center, 0.5));
        assert_eq!(beam_alpha(rect, behind_light, toward_center, 1.0), 0);
    }
}
