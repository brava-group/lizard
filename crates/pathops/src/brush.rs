//! Brushes: strokes drawn with a shape instead of a band. A stretch brush bends one shape along
//! each subpath; a scatter brush drops copies of a shape along it, spaced by the stroke weight and
//! jittered in size, rotation and distance from the path by a seeded (repeatable) sequence. The
//! shapes are the built-in presets below, drawn in code. A width profile (pen pressure) scales the
//! brush along the path. The result may overlap itself; fill it with the non-zero rule.

use crate::width::{Polyline, polylines};
use lizard_doc::{Brush, StrokeProps};
use lizard_geom::kurbo::{self, Affine, BezPath, PathEl, Point, Vec2};

/// What a preset does with its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushKind {
    Stretch,
    Scatter,
}

/// A built-in brush: id (saved in files), label, kind.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: BrushKind,
}

/// The built-in brushes, stretch first.
pub const PRESETS: &[Preset] = &[
    Preset { id: "heist", label: "Heist", kind: BrushKind::Stretch },
    Preset { id: "blockbuster", label: "Blockbuster", kind: BrushKind::Stretch },
    Preset { id: "grindhouse", label: "Grindhouse", kind: BrushKind::Stretch },
    Preset { id: "biopic", label: "Biopic", kind: BrushKind::Stretch },
    Preset { id: "spaghetti_western", label: "Spaghetti Western", kind: BrushKind::Stretch },
    Preset { id: "slasher", label: "Slasher", kind: BrushKind::Stretch },
    Preset { id: "hardboiled", label: "Hardboiled", kind: BrushKind::Stretch },
    Preset { id: "verite", label: "Vérité", kind: BrushKind::Stretch },
    Preset { id: "epic", label: "Epic", kind: BrushKind::Stretch },
    Preset { id: "screwball", label: "Screwball", kind: BrushKind::Stretch },
    Preset { id: "rom_com", label: "Rom Com", kind: BrushKind::Stretch },
    Preset { id: "noir", label: "Noir", kind: BrushKind::Stretch },
    Preset { id: "propaganda", label: "Propaganda", kind: BrushKind::Stretch },
    Preset { id: "melodrama", label: "Melodrama", kind: BrushKind::Stretch },
    Preset { id: "new_wave", label: "New Wave", kind: BrushKind::Stretch },
    Preset { id: "bubblegum", label: "Bubblegum", kind: BrushKind::Scatter },
    Preset { id: "witch_house", label: "Witch House", kind: BrushKind::Scatter },
    Preset { id: "shoegaze", label: "Shoegaze", kind: BrushKind::Scatter },
    Preset { id: "honky_tonk", label: "Honky Tonk", kind: BrushKind::Scatter },
    Preset { id: "screamo", label: "Screamo", kind: BrushKind::Scatter },
    Preset { id: "drone", label: "Drone", kind: BrushKind::Scatter },
    Preset { id: "doo_wop", label: "Doo Wop", kind: BrushKind::Scatter },
    Preset { id: "spoken_word", label: "Spoken Word", kind: BrushKind::Scatter },
    Preset { id: "vaporwave", label: "Vaporwave", kind: BrushKind::Scatter },
    Preset { id: "oi", label: "Oi", kind: BrushKind::Scatter },
    Preset { id: "taper", label: "Taper", kind: BrushKind::Stretch },
    Preset { id: "ink", label: "Ink", kind: BrushKind::Stretch },
    Preset { id: "ribbon", label: "Ribbon", kind: BrushKind::Stretch },
    Preset { id: "bristle", label: "Bristle", kind: BrushKind::Stretch },
    Preset { id: "dots", label: "Dots", kind: BrushKind::Scatter },
    Preset { id: "stars", label: "Stars", kind: BrushKind::Scatter },
    Preset { id: "confetti", label: "Confetti", kind: BrushKind::Scatter },
    Preset { id: "leaves", label: "Leaves", kind: BrushKind::Scatter },
];

/// The most copies a scatter brush makes along a whole path; denser spacing spreads them out.
pub const MAX_COPIES: usize = 5_000;
/// Along-path steps of a stretched shape, per subpath.
const STRETCH_STEPS: usize = 512;
/// Along-path steps of a stretched shape over the whole path (many subpaths get fewer each).
const MAX_STRETCH_STEPS: usize = 65_536;

/// The built-in brush `id`.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// A preset's shape: stretch shapes span x 0..1 along the path and y -0.5..0.5 across it (one
/// stroke weight); scatter shapes are about one unit wide, centred on the origin, pointing along x.
fn shape(id: &str) -> Option<BezPath> {
    let mut p = BezPath::new();
    if let Some(index) = PRESETS[..15].iter().position(|preset| preset.id == id) {
        styled_stretch(&mut p, index);
        return Some(p);
    }
    if let Some(index) = PRESETS[15..25].iter().position(|preset| preset.id == id) {
        styled_scatter(&mut p, index);
        return Some(p);
    }
    match id {
        "taper" => {
            p.move_to((0.0, 0.0));
            p.curve_to((0.3, -0.5), (0.7, -0.5), (1.0, 0.0));
            p.curve_to((0.7, 0.5), (0.3, 0.5), (0.0, 0.0));
            p.close_path();
        }
        // A blunt start, uneven edges and a long tail.
        "ink" => band(&mut p, |x| {
            let w = (x / 0.04).min(1.0) * ((1.0 - x) / 0.3).min(1.0);
            (-0.5 * w + 0.04 * (9.0 * std::f64::consts::PI * x).sin() * w, 0.45 * w + 0.04 * (13.0 * std::f64::consts::PI * x + 1.0).sin() * w)
        }),
        // Edges that cross three times: a twisting ribbon.
        "ribbon" => band(&mut p, |x| {
            let c = 0.5 * (3.0 * std::f64::consts::PI * x).cos();
            (-c, c)
        }),
        // Four thin strands with ragged ends.
        "bristle" => {
            for k in 0..4 {
                let (k, y) = (f64::from(k), -0.375 + 0.25 * f64::from(k));
                let (x0, x1) = (0.03 * k, 1.0 - 0.04 * (3.0 - k));
                let mut s = BezPath::new();
                band(&mut s, |x| {
                    let u = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                    let h = 0.09 * (std::f64::consts::PI * u).sin().max(0.0).sqrt();
                    (y - h, y + h)
                });
                p.extend(s);
            }
        }
        "dots" => p.extend(kurbo::Shape::path_elements(&kurbo::Circle::new((0.0, 0.0), 0.5), 0.01)),
        "stars" => {
            let pts = lizard_geom::star_points(1.0, 1.0, 5, 0.4);
            for (i, q) in pts.iter().enumerate() {
                let q = Point::new(q.x - 0.5, q.y - 0.5);
                if i == 0 { p.move_to(q) } else { p.line_to(q) }
            }
            p.close_path();
        }
        "confetti" => p.extend(kurbo::Shape::path_elements(&kurbo::Rect::new(-0.3, -0.3, 0.3, 0.3), 0.01)),
        "leaves" => {
            p.move_to((-0.5, 0.0));
            p.curve_to((-0.2, -0.3), (0.2, -0.3), (0.5, 0.0));
            p.curve_to((0.2, 0.3), (-0.2, 0.3), (-0.5, 0.0));
            p.close_path();
        }
        _ => return None,
    }
    Some(p)
}

// Original procedural silhouettes. The names match the reference's categories, while the
// geometry remains ours and is shared by the canvas renderer and the selector previews.
fn styled_stretch(p: &mut BezPath, style: usize) {
    use std::f64::consts::PI;
    if style == 4 || style == 5 || style == 7 || style == 14 {
        let count = match style { 4 => 4, 5 => 3, 7 => 6, _ => 3 };
        for strand in 0..count {
            let y = (strand as f64 - (count - 1) as f64 / 2.0) * (0.7 / count as f64);
            band(p, |x| {
                let wave = 0.085 * (2.0 * PI * (1.5 * x + strand as f64 * 0.07)).sin();
                let taper = (x / 0.06).min(1.0) * ((1.0 - x) / 0.07).min(1.0);
                let half = 0.038 * taper * (1.0 + 0.3 * (52.0 * x + strand as f64).sin());
                (y + wave - half, y + wave + half)
            });
        }
        return;
    }
    let rough = match style { 0 => 0.065, 1 => 0.02, 2 => 0.085, 3 => 0.005, 6 => 0.07, 8 => 0.012, 9 => 0.055, 10 => 0.025, 11 => 0.045, 12 => 0.075, 13 => 0.01, _ => 0.03 };
    let thickness = match style { 1 | 8 | 12 => 0.44, 3 | 10 | 13 => 0.30, _ => 0.36 };
    band(p, |x| {
        let fade_in = (x / if style == 3 { 0.13 } else { 0.035 }).min(1.0);
        let fade_out = ((1.0 - x) / if style == 0 || style == 3 { 0.12 } else { 0.04 }).min(1.0);
        let taper = fade_in * fade_out;
        let pulse = 1.0 + 0.17 * (2.0 * PI * (2.0 + style as f64 * 0.12) * x).sin();
        let center = if style == 2 || style == 9 { 0.09 * (2.0 * PI * 2.0 * x).sin() } else { 0.0 };
        let top = rough * ((73.0 * x + style as f64).sin() * 0.6 + (131.0 * x).sin() * 0.4);
        let bottom = rough * ((81.0 * x + style as f64).cos() * 0.6 + (147.0 * x).sin() * 0.4);
        ((center + (-thickness * pulse + top) * taper).clamp(-0.5, 0.5),
         (center + (thickness * pulse + bottom) * taper).clamp(-0.5, 0.5))
    });
    if matches!(style, 0 | 2 | 6 | 9 | 11 | 12) {
        for strand in 0..3 {
            let y = (strand as f64 - 1.0) * 0.15;
            band(p, |x| {
                let span = (x / 0.07).min(1.0) * ((1.0 - x) / (0.06 + strand as f64 * 0.04)).min(1.0);
                let jitter = 0.025 * (83.0 * x + strand as f64 * 2.0).sin();
                (y + jitter - 0.018 * span, y + jitter + 0.018 * span)
            });
        }
    }
}

fn styled_scatter(p: &mut BezPath, style: usize) {
    use std::f64::consts::PI;
    let points = match style { 1 | 4 => 5, 2 | 8 => 8, 3 | 6 => 4, 7 => 3, 5 => 12, 9 => 10, _ => 16 };
    let inner = match style { 1 => 0.43, 2 => 0.68, 3 => 0.78, 4 => 0.25, 6 => 0.35, 7 => 0.3, 8 => 0.4, _ => 1.0 };
    let n = if inner < 1.0 { points * 2 } else { points };
    for i in 0..n {
        let a = 2.0 * PI * i as f64 / n as f64;
        let radius = if inner < 1.0 && i % 2 == 1 { 0.5 * inner } else { 0.5 };
        let point = (radius * a.cos(), radius * a.sin() * if style == 5 { 0.62 } else { 1.0 });
        if i == 0 { p.move_to(point) } else { p.line_to(point) }
    }
    p.close_path();
    if matches!(style, 0 | 5 | 9) {
        // A smaller second mark gives these stamps a distinct rhythm at low zoom.
        let (x, y, size) = match style { 5 => (0.22, 0.24, 0.09), 9 => (0.27, -0.27, 0.14), _ => (-0.24, 0.26, 0.12) };
        p.extend(kurbo::Shape::path_elements(&kurbo::Circle::new((x, y), size), 0.01));
    }
}

/// A closed band from x 0 to 1 between edges `f(x) = (top, bottom)`.
fn band(p: &mut BezPath, f: impl Fn(f64) -> (f64, f64)) {
    const N: usize = 64;
    let xs = (0..=N).map(|i| i as f64 / N as f64);
    for (i, x) in xs.clone().enumerate() {
        let q = (x, f(x).0);
        if i == 0 { p.move_to(q) } else { p.line_to(q) }
    }
    for x in xs.rev() {
        p.line_to((x, f(x).1));
    }
    p.close_path();
}

/// A repeatable pseudo-random number in 0..1 for copy `i`, channel `k` (splitmix64).
fn rand(seed: u32, i: u64, k: u64) -> f64 {
    let mut z = u64::from(seed) ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The stroke of `path` drawn with `brush` at `width` (the profile, if any, scales it along each
/// subpath). `None` for an unknown preset or a path too long to measure.
pub fn outline(path: &BezPath, width: f64, props: &StrokeProps, brush: &Brush) -> Option<BezPath> {
    let p = preset(&brush.preset)?;
    let unit = shape(p.id)?;
    let lines = polylines(path)?;
    let scale = |t: f64| width * props.profile().map_or(1.0, |pr| pr.scale_at(t));
    let mut out = BezPath::new();
    match p.kind {
        BrushKind::Stretch => {
            let steps = (MAX_STRETCH_STEPS / lines.len().max(1)).clamp(8, STRETCH_STEPS);
            for line in &lines {
                stretch(&mut out, &unit, line, steps, &scale);
            }
        }
        BrushKind::Scatter => scatter(&mut out, &unit, &lines, width, brush, &scale),
    }
    Some(out)
}

/// `unit` bent along `line` in at most `steps` steps along it: x by length, y across at the scaled
/// width.
fn stretch(out: &mut BezPath, unit: &BezPath, line: &Polyline, steps: usize, scale: &dyn Fn(f64) -> f64) {
    let map = |q: Point| line.at(q.x).map(|(p, n)| p + n * (q.y * scale(q.x)));
    let mut last = Point::ZERO;
    kurbo::flatten(unit.iter(), 0.002, |el| match el {
        PathEl::MoveTo(q) => {
            last = q;
            if let Some(m) = map(q) {
                out.move_to(m);
            }
        }
        PathEl::LineTo(q) => {
            // Short steps along x so the shape bends with the path.
            let n = (((q.x - last.x).abs() * steps as f64).ceil() as usize).clamp(1, steps);
            for k in 1..=n {
                if let Some(m) = map(last.lerp(q, k as f64 / n as f64)) {
                    out.line_to(m);
                }
            }
            last = q;
        }
        PathEl::ClosePath => out.close_path(),
        _ => {}
    });
}

/// Copies of `unit` along `lines`, `brush.spacing` weights apart, jittered.
fn scatter(out: &mut BezPath, unit: &BezPath, lines: &[Polyline], width: f64, brush: &Brush, scale: &dyn Fn(f64) -> f64) {
    let total: f64 = lines.iter().map(Polyline::length).sum();
    let b = brush.sanitized();
    let step = (b.spacing * width).max(total / MAX_COPIES as f64).max(1e-3);
    let mut i = 0u64;
    for line in lines {
        let len = line.length();
        let mut s = step / 2.0;
        while s < len {
            let t = s / len;
            if let Some((p, n)) = line.at(t) {
                let r = |k| rand(b.seed, i, k) * 2.0 - 1.0;
                let size = scale(t) * (1.0 + b.size_jitter * r(0)).max(0.05);
                let dir = Vec2::new(n.y, -n.x);
                let angle = dir.atan2() + b.rotation_jitter * std::f64::consts::PI * r(1);
                let at = p + n * (b.scatter * width * r(2));
                let xf = Affine::translate(at.to_vec2()) * Affine::rotate(angle) * Affine::scale(size);
                out.extend(unit.iter().map(|el| xf * el));
            }
            i += 1;
            s += step;
        }
    }
}
