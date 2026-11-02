//! Local comparison of the currently shipped cursor images and independent vector proposals.
//! The proposals are drawn only in this catalog; they do not change production cursors.

use std::collections::HashMap;

use egui::{Color32, ColorImage, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2};
use lizard_ui_egui::primitives::TooltipResponseExt as _;

struct CursorSample {
    key: &'static str,
    label: &'static str,
    group: &'static str,
    image: &'static [u8],
    old_hotspot: [f32; 2],
    new_hotspot: [f32; 2],
}

macro_rules! sample {
    ($key:literal, $label:literal, $group:literal, $old:expr, $new:expr) => {
        CursorSample {
            key: $key,
            label: $label,
            group: $group,
            image: include_bytes!(concat!("../../../crates/ui-egui/assets/cursors/", $key, ".png")),
            old_hotspot: $old,
            new_hotspot: $new,
        }
    };
}

const SAMPLES: &[CursorSample] = &[
    sample!("move", "Move", "Pointer", [0.0, 0.0], [4.0, 3.0]),
    sample!("click", "Click", "Pointer", [0.0, 0.0], [13.0, 3.0]),
    sample!("duplicate", "Duplicate", "Pointer", [0.0, 0.0], [3.0, 6.0]),
    sample!("crosshair", "Crosshair", "Pointer", [16.0, 16.0], [16.0, 16.0]),
    sample!("resize-horizontal", "Resize horizontal", "Transform", [16.0, 16.0], [16.0, 16.0]),
    sample!("resize-vertical", "Resize vertical", "Transform", [16.0, 16.0], [16.0, 16.0]),
    sample!("resize-diagonal-dr", "Resize diagonal ↘", "Transform", [16.0, 16.0], [16.0, 16.0]),
    sample!("resize-diagonal-ur", "Resize diagonal ↗", "Transform", [16.0, 16.0], [16.0, 16.0]),
    sample!("rotate", "Rotate", "Transform", [16.0, 16.0], [16.0, 16.0]),
    sample!("hand", "Pan", "Navigation", [16.0, 16.0], [16.0, 16.0]),
    sample!("hand-press", "Panning", "Navigation", [16.0, 16.0], [16.0, 16.0]),
    sample!("zoom-in", "Zoom in", "Navigation", [16.0, 16.0], [14.0, 14.0]),
    sample!("zoom-out", "Zoom out", "Navigation", [16.0, 16.0], [14.0, 14.0]),
    sample!("pen", "Pen", "Vector", [0.0, 0.0], [2.0, 2.0]),
    sample!("pen-dot", "Close path", "Vector", [0.0, 0.0], [2.0, 2.0]),
    sample!("pen-insert", "Insert point", "Vector", [0.0, 0.0], [2.0, 2.0]),
    sample!("pen-snap", "Snap point", "Vector", [16.0, 16.0], [2.0, 2.0]),
    sample!("bend", "Bend", "Vector", [16.0, 16.0], [16.0, 16.0]),
    sample!("bend-curve", "Bend curve", "Vector", [16.0, 16.0], [16.0, 16.0]),
    sample!("lasso", "Lasso", "Vector", [0.0, 0.0], [3.0, 3.0]),
    sample!("cut", "Cut", "Vector", [0.0, 0.0], [16.0, 16.0]),
    sample!("eraser", "Eraser", "Vector", [16.0, 16.0], [16.0, 16.0]),
    sample!("paint-bucket", "Paint", "Paint", [8.0, 24.0], [12.0, 23.0]),
    sample!("paint-bucket-insert", "Paint add", "Paint", [8.0, 24.0], [12.0, 23.0]),
    sample!("paint-bucket-remove", "Paint remove", "Paint", [8.0, 24.0], [12.0, 23.0]),
    sample!("eyedropper", "Eyedropper", "Paint", [5.0, 27.0], [7.0, 25.0]),
    sample!("comment", "Comment", "Other", [6.0, 27.0], [9.0, 24.0]),
];

const PROPOSAL_SCALE: f32 = 0.80;

pub struct State {
    query: String,
    light_canvas: bool,
    show_hotspots: bool,
    enlarged: bool,
    textures: HashMap<&'static str, TextureHandle>,
}

impl Default for State {
    fn default() -> Self {
        Self { query: String::new(), light_canvas: false, show_hotspots: false, enlarged: true, textures: HashMap::new() }
    }
}

impl State {
    fn old_texture(&mut self, ctx: &egui::Context, sample: &CursorSample) -> Option<&TextureHandle> {
        if !self.textures.contains_key(sample.key) {
            let rgba = image::load_from_memory_with_format(sample.image, image::ImageFormat::Png).ok()?.into_rgba8();
            let color = ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
            let texture = ctx.load_texture(format!("cursor-current-{}", sample.key), color, TextureOptions::LINEAR);
            self.textures.insert(sample.key, texture);
        }
        self.textures.get(sample.key)
    }
}

pub fn show(ui: &mut Ui, state: &mut State) {
    ui.label("Compare the current cursor with a separate, code-drawn Lizard proposal. Nothing here changes the editor.");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.query).hint_text("Filter cursors").desired_width(200.0));
        ui.checkbox(&mut state.light_canvas, "Light canvas");
        ui.checkbox(&mut state.show_hotspots, "Click points");
        ui.checkbox(&mut state.enlarged, "2× preview");
    });
    ui.add_space(10.0);
    let query = state.query.trim().to_lowercase();
    let background = if state.light_canvas { Color32::from_rgb(244, 246, 244) } else { Color32::from_rgb(30, 34, 32) };
    let show_hotspots = state.show_hotspots;
    let side = if state.enlarged { 64.0 } else { 32.0 };
    let mut last_group = "";
    egui::Grid::new("cursor_comparison").num_columns(3).spacing(vec2(12.0, 10.0)).show(ui, |ui| {
        ui.strong("State");
        ui.strong("Current");
        ui.strong("Lizard proposal");
        ui.end_row();
        for sample in SAMPLES {
            if !query.is_empty() && !sample.label.to_lowercase().contains(&query) && !sample.group.to_lowercase().contains(&query) {
                continue;
            }
            if sample.group != last_group {
                last_group = sample.group;
                ui.strong(sample.group);
                ui.label("");
                ui.label("");
                ui.end_row();
            }
            ui.label(sample.label);
            let old = preview_cell(ui, background, side);
            if let Some(texture) = state.old_texture(ui.ctx(), sample) {
                let square = centered_square(old, side);
                ui.painter().image(texture.id(), square, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                if show_hotspots {
                    hotspot(ui, square, sample.old_hotspot, 1.0);
                }
            } else {
                ui.painter().text(old.center(), egui::Align2::CENTER_CENTER, "Image unavailable", egui::FontId::proportional(11.0), Color32::GRAY);
            }
            let proposal = preview_cell(ui, background, side);
            let square = centered_square(proposal, side);
            draw_proposal(ui, square, sample.key);
            if show_hotspots {
                hotspot(ui, square, sample.new_hotspot, PROPOSAL_SCALE);
            }
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.small(
        "Current images are shown only for local review. The vector proposals are original catalog artwork and have not been applied to production.",
    );
}

fn preview_cell(ui: &mut Ui, background: Color32, side: f32) -> Rect {
    let (rect, response) = ui.allocate_exact_size(vec2(190.0, side + 24.0), Sense::hover());
    ui.painter().rect(rect, 8.0, background, Stroke::new(1.0, Color32::from_gray(95)), StrokeKind::Inside);
    response.lizard_tooltip("Artwork at its intended cursor size");
    rect
}

fn centered_square(cell: Rect, side: f32) -> Rect {
    Rect::from_center_size(cell.center(), vec2(side, side))
}

fn hotspot(ui: &Ui, square: Rect, point: [f32; 2], artwork_scale: f32) {
    let x = 16.0 + (point[0] - 16.0) * artwork_scale;
    let y = 16.0 + (point[1] - 16.0) * artwork_scale;
    let position = square.min + vec2(x, y) * (square.width() / 32.0);
    ui.painter().circle_filled(position, 3.0, Color32::from_rgb(0, 154, 255));
    ui.painter().circle_stroke(position, 3.0, Stroke::new(1.0, Color32::WHITE));
}

struct Drawing<'a> {
    painter: &'a egui::Painter,
    square: Rect,
    ink: Color32,
    edge: Color32,
    accent: Color32,
}

impl Drawing<'_> {
    fn point(&self, x: f32, y: f32) -> Pos2 {
        let x = 16.0 + (x - 16.0) * PROPOSAL_SCALE;
        let y = 16.0 + (y - 16.0) * PROPOSAL_SCALE;
        self.square.min + vec2(x, y) * (self.square.width() / 32.0)
    }

    fn width(&self, width: f32) -> f32 {
        width * PROPOSAL_SCALE * self.square.width() / 32.0
    }

    fn path(&self, points: &[(f32, f32)]) {
        let path: Vec<_> = points.iter().map(|&(x, y)| self.point(x, y)).collect();
        self.painter.add(Shape::line(path, Stroke::new(self.width(2.1), self.edge)));
    }

    fn filled_polygon(&self, points: &[(f32, f32)]) {
        let points = points.iter().map(|&(x, y)| self.point(x, y)).collect();
        self.painter.add(Shape::convex_polygon(points, self.ink, Stroke::NONE));
    }

    fn filled_round_rect(&self, min: (f32, f32), max: (f32, f32), radius: f32) {
        self.painter.rect_filled(Rect::from_min_max(self.point(min.0, min.1), self.point(max.0, max.1)), self.width(radius), self.ink);
    }

    fn circle(&self, x: f32, y: f32, radius: f32) {
        let point = self.point(x, y);
        self.painter.circle_filled(point, self.width(radius + 1.0), self.edge);
        self.painter.circle_filled(point, self.width(radius), self.accent);
        self.painter.circle_stroke(point, self.width(radius), Stroke::new(self.width(1.0), self.ink));
    }

    fn badge(&self, kind: &str) {
        self.circle(25.0, 25.0, 5.0);
        match kind {
            "add" => {
                self.path(&[(22.5, 25.0), (27.5, 25.0)]);
                self.path(&[(25.0, 22.5), (25.0, 27.5)]);
            }
            "remove" => self.path(&[(22.5, 25.0), (27.5, 25.0)]),
            "close" => self.circle(25.0, 25.0, 1.4),
            "snap" => self.path(&[(22.7, 25.0), (24.3, 26.6), (27.5, 23.2)]),
            _ => {}
        }
    }
}

fn arrow(d: &Drawing<'_>, offset: (f32, f32), outline: Color32) {
    let point = |x: f32, y: f32| (x + offset.0, y + offset.1);
    // Two convex fills produce a single angular pointer without copied bitmap geometry.
    d.filled_polygon(&[point(4.0, 3.0), point(4.0, 24.0), point(21.0, 15.0)]);
    d.filled_polygon(&[point(9.0, 18.0), point(13.0, 16.0), point(19.0, 24.0), point(15.0, 27.0)]);
    let perimeter = [
        point(4.0, 3.0),
        point(4.0, 24.0),
        point(9.0, 18.0),
        point(15.0, 27.0),
        point(19.0, 24.0),
        point(13.0, 16.0),
        point(21.0, 15.0),
        point(4.0, 3.0),
    ];
    let points = perimeter.into_iter().map(|(x, y)| d.point(x, y)).collect();
    d.painter.add(Shape::line(points, Stroke::new(d.width(2.0), outline)));
}

fn click_hand(d: &Drawing<'_>) {
    d.filled_round_rect((9.0, 14.0), (26.0, 27.0), 5.0);
    d.filled_round_rect((11.0, 3.0), (16.0, 19.0), 2.5);
    d.filled_round_rect((16.0, 10.0), (20.0, 19.0), 2.0);
    d.filled_round_rect((20.0, 12.0), (24.0, 20.0), 2.0);
    d.filled_round_rect((23.0, 15.0), (27.0, 21.0), 2.0);
    d.filled_polygon(&[(10.0, 21.0), (5.0, 16.0), (6.0, 12.0), (9.0, 12.0), (15.0, 18.0)]);
    d.path(&[
        (11.0, 16.0),
        (11.0, 6.0),
        (11.4, 4.4),
        (12.5, 3.2),
        (14.0, 3.0),
        (15.4, 4.0),
        (16.0, 6.0),
        (16.0, 14.0),
        (16.6, 11.0),
        (17.0, 10.0),
        (19.0, 10.0),
        (20.0, 11.5),
        (20.0, 13.0),
        (20.8, 12.1),
        (21.0, 12.0),
        (23.0, 12.0),
        (23.4, 12.7),
        (24.0, 16.0),
        (25.0, 15.7),
        (26.0, 16.0),
        (27.0, 17.5),
        (27.0, 21.0),
        (26.0, 23.5),
        (24.0, 26.0),
        (21.0, 27.0),
        (14.0, 27.0),
        (11.4, 25.5),
        (10.0, 24.0),
        (6.0, 19.0),
        (5.0, 17.0),
        (5.0, 14.5),
        (6.0, 12.7),
        (7.0, 12.0),
        (7.5, 12.0),
        (9.0, 13.0),
        (13.0, 18.0),
    ]);
}

fn resize(d: &Drawing<'_>, angle: f32) {
    let (dx, dy) = (angle.to_radians().cos(), angle.to_radians().sin());
    let (sx, sy) = (-dy, dx);
    let point = |along: f32, across: f32| (16.0 + dx * along + sx * across, 16.0 + dy * along + sy * across);
    d.path(&[point(-10.0, 0.0), point(10.0, 0.0)]);
    for sign in [-1.0, 1.0] {
        let tip = point(sign * 11.0, 0.0);
        d.path(&[point(sign * 5.0, -4.0), tip, point(sign * 5.0, 4.0)]);
    }
    d.circle(16.0, 16.0, 1.2);
}

fn pen(d: &Drawing<'_>) {
    d.path(&[(2.0, 2.0), (10.0, 4.0), (22.0, 16.0), (14.0, 22.0), (7.0, 12.0), (2.0, 2.0)]);
    d.path(&[(7.0, 12.0), (13.0, 13.0), (14.0, 22.0)]);
    d.circle(13.0, 13.0, 1.3);
}

fn bucket(d: &Drawing<'_>) {
    d.path(&[(5.0, 9.0), (19.0, 6.0), (25.0, 17.0), (13.0, 23.0), (5.0, 9.0)]);
    d.path(&[(9.0, 16.0), (22.0, 14.0)]);
    d.circle(12.0, 23.0, 1.3);
}

fn draw_proposal(ui: &Ui, square: Rect, key: &str) {
    let d =
        Drawing { painter: ui.painter(), square, ink: Color32::from_rgb(17, 32, 28), edge: Color32::WHITE, accent: Color32::from_rgb(98, 230, 169) };
    match key {
        "move" => arrow(&d, (0.0, 0.0), d.edge),
        "click" => click_hand(&d),
        "duplicate" => {
            arrow(&d, (3.0, -3.0), Color32::from_gray(150));
            arrow(&d, (-1.0, 3.0), d.edge);
        }
        "crosshair" => {
            d.path(&[(16.0, 3.0), (16.0, 11.0)]);
            d.path(&[(16.0, 21.0), (16.0, 29.0)]);
            d.path(&[(3.0, 16.0), (11.0, 16.0)]);
            d.path(&[(21.0, 16.0), (29.0, 16.0)]);
            d.circle(16.0, 16.0, 2.2);
        }
        "resize-horizontal" => resize(&d, 0.0),
        "resize-vertical" => resize(&d, 90.0),
        "resize-diagonal-dr" => resize(&d, 45.0),
        "resize-diagonal-ur" => resize(&d, -45.0),
        "rotate" => {
            let arc: Vec<_> = (0..=15)
                .map(|i| {
                    let angle = (-160.0 + i as f32 * 280.0 / 15.0).to_radians();
                    (16.0 + 10.0 * angle.cos(), 16.0 + 10.0 * angle.sin())
                })
                .collect();
            d.path(&arc);
            d.path(&[(11.0, 23.0), (11.0, 28.0), (17.0, 27.0)]);
            d.circle(16.0, 16.0, 1.2);
        }
        "hand" | "hand-press" => {
            d.path(&[
                (10.0, 26.0),
                (7.0, 18.0),
                (9.0, 15.0),
                (13.0, 19.0),
                (12.0, 7.0),
                (15.0, 5.0),
                (17.0, 16.0),
                (18.0, 7.0),
                (21.0, 6.0),
                (22.0, 17.0),
                (24.0, 10.0),
                (27.0, 10.0),
                (27.0, 21.0),
                (23.0, 27.0),
                (10.0, 26.0),
            ]);
            if key == "hand-press" {
                d.path(&[(12.0, 21.0), (22.0, 21.0)]);
            }
        }
        "zoom-in" | "zoom-out" => {
            let ring: Vec<_> = (0..=18)
                .map(|i| {
                    let angle = i as f32 * std::f32::consts::TAU / 18.0;
                    (14.0 + 8.0 * angle.cos(), 14.0 + 8.0 * angle.sin())
                })
                .collect();
            d.path(&ring);
            d.path(&[(20.0, 20.0), (28.0, 28.0)]);
            d.path(&[(10.0, 14.0), (18.0, 14.0)]);
            if key == "zoom-in" {
                d.path(&[(14.0, 10.0), (14.0, 18.0)]);
            }
        }
        "pen" | "pen-dot" | "pen-insert" | "pen-snap" => {
            pen(&d);
            match key {
                "pen-dot" => d.badge("close"),
                "pen-insert" => d.badge("add"),
                "pen-snap" => d.badge("snap"),
                _ => {}
            }
        }
        "bend" | "bend-curve" => {
            d.path(&[(4.0, 23.0), (10.0, 12.0), (18.0, 12.0), (27.0, 6.0)]);
            d.circle(4.0, 23.0, 2.0);
            d.circle(27.0, 6.0, 2.0);
            if key == "bend-curve" {
                d.path(&[(12.0, 21.0), (19.0, 20.0), (22.0, 15.0)]);
            } else {
                d.circle(17.0, 12.0, 2.0);
            }
        }
        "lasso" => {
            let loop_points: Vec<_> = (0..=22)
                .map(|i| {
                    let angle = i as f32 * std::f32::consts::TAU / 22.0;
                    (16.0 + 10.0 * angle.cos(), 14.0 + 9.0 * angle.sin())
                })
                .collect();
            d.path(&loop_points);
            d.path(&[(13.0, 21.0), (8.0, 28.0), (5.0, 27.0)]);
            d.circle(3.0, 3.0, 1.1);
        }
        "cut" => {
            d.path(&[(8.0, 7.0), (24.0, 25.0)]);
            d.path(&[(24.0, 7.0), (8.0, 25.0)]);
            d.circle(7.0, 7.0, 3.0);
            d.circle(25.0, 7.0, 3.0);
            d.circle(16.0, 16.0, 1.0);
        }
        "eraser" => {
            d.path(&[(7.0, 20.0), (18.0, 5.0), (27.0, 13.0), (17.0, 27.0), (7.0, 20.0)]);
            d.path(&[(12.0, 15.0), (22.0, 22.0)]);
        }
        "paint-bucket" | "paint-bucket-insert" | "paint-bucket-remove" => {
            bucket(&d);
            if key == "paint-bucket-insert" {
                d.badge("add");
            }
            if key == "paint-bucket-remove" {
                d.badge("remove");
            }
        }
        "eyedropper" => {
            d.path(&[(7.0, 25.0), (12.0, 17.0), (19.0, 9.0), (23.0, 12.0), (16.0, 20.0), (7.0, 25.0)]);
            d.path(&[(19.0, 9.0), (22.0, 5.0), (27.0, 9.0), (23.0, 12.0)]);
            d.circle(7.0, 25.0, 1.5);
        }
        "comment" => {
            d.path(&[
                (8.0, 8.0),
                (25.0, 8.0),
                (27.0, 11.0),
                (27.0, 21.0),
                (22.0, 24.0),
                (15.0, 24.0),
                (9.0, 28.0),
                (10.0, 22.0),
                (6.0, 19.0),
                (6.0, 11.0),
                (8.0, 8.0),
            ]);
            d.circle(12.0, 16.0, 1.1);
            d.circle(17.0, 16.0, 1.1);
            d.circle(22.0, 16.0, 1.1);
        }
        _ => {}
    }
}
