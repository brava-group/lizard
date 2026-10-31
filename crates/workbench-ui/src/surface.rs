//! Shared rounded chrome for menus and compact overlay panels.

use egui::{Color32, CornerRadius, Frame, InnerResponse, Painter, Rect, Shadow, Shape, Ui, pos2, vec2};

/// Colors supplied by the host app; Workbench UI does not depend on editor state.
#[derive(Clone, Copy)]
pub struct SurfaceTheme {
    pub dark: bool,
    pub panel: Color32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceVariant {
    Menu,
    Palette,
}

/// The visual shell only. Callers compose the contents and own popup behavior.
pub struct Surface {
    variant: SurfaceVariant,
}

impl Surface {
    pub fn menu() -> Self {
        Self { variant: SurfaceVariant::Menu }
    }

    pub fn palette() -> Self {
        Self { variant: SurfaceVariant::Palette }
    }

    pub fn frame(&self, theme: SurfaceTheme) -> Frame {
        match self.variant {
            SurfaceVariant::Menu => Frame::NONE
                .fill(Color32::from_rgb(0x1e, 0x1e, 0x1e))
                .corner_radius(CornerRadius::same(13))
                .inner_margin(egui::Margin::symmetric(0, 8))
                .shadow(Shadow { offset: [0, 10], blur: 14, spread: 0, color: Color32::from_black_alpha(89) }),
            SurfaceVariant::Palette => Frame::NONE
                .fill(if theme.dark { Color32::from_rgb(0x2c, 0x2c, 0x2c) } else { theme.panel })
                .corner_radius(CornerRadius::same(13))
                .inner_margin(egui::Margin::same(8))
                .shadow(Shadow { offset: [0, 3], blur: 7, spread: 0, color: Color32::from_black_alpha(if theme.dark { 89 } else { 45 }) }),
        }
    }

    /// Paint both outer shadows behind the surface and both inner highlights above it.
    pub fn show<R>(&self, ui: &mut Ui, theme: SurfaceTheme, content: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let second_shadow = ui.painter().add(Shape::Noop);
        let response = self.frame(theme).show(ui, content);
        let rect = response.response.rect;
        ui.painter().set(second_shadow, self.close_shadow().as_shape(rect, CornerRadius::same(13)));
        self.paint_top_highlight(ui.painter(), rect);
        response
    }

    fn close_shadow(&self) -> Shadow {
        match self.variant {
            // CSS reference: 0 2px 4.375px #00000059. egui stores blur in whole pixels.
            SurfaceVariant::Menu => Shadow { offset: [0, 2], blur: 4, spread: 0, color: Color32::from_black_alpha(89) },
            // CSS reference: 0 1px 2.625px #00000080; egui rounds the blur to 3 points.
            SurfaceVariant::Palette => Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(128) },
        }
    }

    /// A Popup owns its egui frame, so its near shadow is painted on its layer outside the frame.
    pub fn paint_close_shadow_outside(&self, painter: &Painter, rect: Rect) {
        let bounds = rect.expand(16.0);
        let outside = [
            Rect::from_min_max(bounds.min, pos2(bounds.right(), rect.top())),
            Rect::from_min_max(pos2(bounds.left(), rect.top()), pos2(rect.left(), rect.bottom())),
            Rect::from_min_max(pos2(rect.right(), rect.top()), pos2(bounds.right(), rect.bottom())),
            Rect::from_min_max(pos2(bounds.left(), rect.bottom()), bounds.max),
        ];
        for clip in outside {
            painter.with_clip_rect(clip).add(self.close_shadow().as_shape(rect, CornerRadius::same(13)));
        }
    }

    pub fn paint_top_highlight(&self, painter: &Painter, rect: Rect) {
        if rect.width() <= 26.0 {
            return;
        }
        match self.variant {
            SurfaceVariant::Menu => {
                // Slightly quieter than the CSS reference; both bands stay within 1 pt.
                paint_top_inset(painter, rect, 0.0, 80, 1.0);
                paint_top_inset(painter, rect, 0.5, 18, 1.0);
            }
            SurfaceVariant::Palette => {
                // Slightly quieter than the CSS reference; both bands stay within 1 pt.
                paint_top_inset(painter, rect, 0.0, 69, 1.0);
                paint_top_inset(painter, rect, 0.5, 18, 1.0);
            }
        }
    }
}

/// A connected gradient band preserves a continuous tangent and tapers down the sides.
/// Its 1 pt total width gives the rasterizer enough coverage to keep curves smooth.
fn paint_top_inset(painter: &Painter, rect: Rect, inset: f32, alpha: u8, width: f32) {
    // The full band includes its soft fringe and never exceeds one logical pixel.
    let pixels_per_point = painter.pixels_per_point();
    let band_width = width.max(0.5).max(1.0 / pixels_per_point).min(1.0);
    let radius = (13.0 - inset).max(0.0);
    let left = rect.left() + 13.0;
    let right = rect.right() - 13.0;
    let top = rect.top() + 13.0;
    // Shift the whole contour by the same amount. Snapping just the tangent
    // points bends the curve into the straight edge and creates a bright knot.
    let y_shift = painter.round_to_pixel_center(top - radius) - (top - radius);
    // Curves get more segments as their on-screen pixel radius grows, while
    // their intermediate points retain subpixel positions for smooth edges.
    let steps = (radius * pixels_per_point * std::f32::consts::FRAC_PI_2 / 2.0).ceil().clamp(12.0, 64.0) as usize;
    let tail = (radius * 0.6).min(8.0);
    let mut contour = Vec::with_capacity(steps * 2 + 4);
    contour.push(pos2(left - radius, top + tail + y_shift));
    for step in 0..steps {
        let angle = std::f32::consts::PI * (1.0 + step as f32 / (steps as f32 * 2.0));
        contour.push(pos2(left + radius * angle.cos(), top + radius * angle.sin() + y_shift));
    }
    let left_tangent = contour.len();
    contour.push(pos2(left, top - radius + y_shift));
    let right_tangent = contour.len();
    contour.push(pos2(right, top - radius + y_shift));
    for step in 1..=steps {
        let angle = std::f32::consts::PI * (1.5 + step as f32 / (steps as f32 * 2.0));
        contour.push(pos2(right + radius * angle.cos(), top + radius * angle.sin() + y_shift));
    }
    contour.push(pos2(right + radius, top + tail + y_shift));
    let mut travel = Vec::with_capacity(contour.len());
    let mut distance = 0.0;
    travel.push(distance);
    for points in contour.windows(2) {
        distance += points[0].distance(points[1]);
        travel.push(distance);
    }
    let mut mesh = egui::Mesh::default();
    let half = band_width * 0.25;
    let fringe = band_width * 0.25;
    let last = contour.len() - 1;
    for (index, point) in contour.iter().enumerate() {
        let tangent = match index {
            0 => contour[1] - contour[0],
            i if i == last => contour[last] - contour[last - 1],
            i => contour[i + 1] - contour[i - 1],
        }
        .normalized();
        let normal = vec2(-tangent.y, tangent.x);
        let progress = if index <= left_tangent {
            travel[index] / travel[left_tangent]
        } else if index <= right_tangent {
            1.0
        } else {
            (travel[last] - travel[index]) / (travel[last] - travel[right_tangent])
        };
        let fade = (progress.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin().powf(1.35);
        let taper = 0.35 + 0.65 * fade;
        let ink = Color32::from_white_alpha((f32::from(alpha) * fade).round() as u8);
        for (offset, color) in [
            (-(half + fringe) * taper, Color32::TRANSPARENT),
            (-half * taper, ink),
            (half * taper, ink),
            ((half + fringe) * taper, Color32::TRANSPARENT),
        ] {
            mesh.colored_vertex(*point + normal * offset, color);
        }
    }
    for index in 0..last {
        let base = (index * 4) as u32;
        for side in 0..3 {
            let start = base + side;
            mesh.add_triangle(start, start + 4, start + 1);
            mesh.add_triangle(start + 1, start + 4, start + 5);
        }
    }
    painter.add(Shape::mesh(mesh));
}
