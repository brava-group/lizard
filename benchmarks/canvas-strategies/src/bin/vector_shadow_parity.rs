//! Compare Vello's generic drop-shadow filter with the current document renderer.

use std::error::Error;

use lizard_canvas_gpu::vello_scene::{GpuCanvas, has_unhandled_effects_in_view};
use lizard_color::{Color, Paint};
use lizard_doc::{Document, Effect, EffectKind, NodeKind, VectorNetwork};
use lizard_geom::{Affine, Point};
use lizard_render::{Options, View, draw, render_view};
use vello_cpu::filter_effects::{EdgeMode, Filter, FilterPrimitive};
use vello_cpu::peniko;
use vello_cpu::{Pixmap, RenderContext, Resources};

fn difference(actual: &[u8], reference: &[u8], width: u32, height: u32) -> (f64, f64, u8) {
    let mut error = 0u64;
    let mut changed = 0usize;
    let mut maximum = 0u8;
    for (a, b) in actual.chunks_exact(4).zip(reference.chunks_exact(4)) {
        let delta = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
        error += delta.iter().map(|value| u64::from(*value)).sum::<u64>();
        changed += usize::from(delta.iter().any(|value| *value > 8));
        maximum = maximum.max(*delta.iter().max().unwrap_or(&0));
    }
    let pixels = u64::from(width) * u64::from(height);
    (error as f64 / (pixels * 3) as f64, changed as f64 * 100.0 / pixels as f64, maximum)
}

fn main() -> Result<(), Box<dyn Error>> {
    let spread = std::env::var("LIZARD_SHADOW_SPREAD").ok().and_then(|value| value.parse::<f64>().ok()).unwrap_or(0.0);
    let fill_alpha = std::env::var("LIZARD_FILL_ALPHA").ok().and_then(|value| value.parse::<f32>().ok()).unwrap_or(1.0);
    let mut doc = Document::new("Vector shadow parity");
    let page = doc.current_page.ok_or("new document has no page")?;
    let network = VectorNetwork::from_points(
        &[Point::new(10.0, 10.0), Point::new(140.0, 25.0), Point::new(95.0, 65.0), Point::new(110.0, 150.0), Point::new(20.0, 140.0)],
        true,
    );
    let id = doc.create(page, NodeKind::Vector { network }, "Irregular vector", 35.0, 30.0, 140.0, 150.0)?;
    let node = doc.node_mut(id).ok_or("vector missing")?;
    node.fills = vec![Paint::solid(Color::new(0.8, 0.8, 0.85, fill_alpha))];
    let mut shadow = Effect::drop_shadow();
    if let EffectKind::DropShadow(ref mut value) = shadow.kind {
        value.radius = 16.0;
        value.offset = (5.0, 9.0);
        value.color = Color::new(0.2, 0.1, 0.3, 0.5);
        value.spread = spread;
    }
    node.effects = vec![shadow];
    let view = View { origin: (0.0, 0.0), scale: 1.0, width: 256, height: 256 };
    let gpu_supported = !has_unhandled_effects_in_view(&doc, &view);
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, background_visible: true, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU render failed")?;
    let mut gpu = GpuCanvas::new(view.width, view.height)?;
    if std::env::var_os("LIZARD_SHADOW_CACHE").is_some() {
        gpu.prepare_static_effects(&doc)?;
    }
    gpu.render_submit_document(&doc, page, &view, clear)?;
    gpu.wait()?;
    let actual = gpu.pixels()?;
    let (mean, changed, maximum) = difference(&actual, reference.data(), view.width, view.height);
    println!(
        "vello_gpu spread={spread} fill_alpha={fill_alpha} supported={gpu_supported} mean_rgb_error={mean:.3}/255 changed_gt8={changed:.3}% max={maximum}"
    );
    if std::env::var_os("LIZARD_RESIZE_PROBE").is_some() {
        let resized = View { width: 320, ..view };
        gpu.resize(resized.width, resized.height);
        gpu.render_submit_document(&doc, page, &resized, clear)?;
        gpu.wait()?;
        let actual = gpu.pixels()?;
        let reference = render_view(&doc, page, &resized, &Options::default()).ok_or("CPU render failed after resize")?;
        let (mean, changed, maximum) = difference(&actual, reference.data(), resized.width, resized.height);
        println!("vello_gpu resized={}x{} mean_rgb_error={mean:.3}/255 changed_gt8={changed:.3}% max={maximum}", resized.width, resized.height);
    }

    // Vello CPU 0.3 uses the same vello_common 0.3 data model as the GPU crate. This small probe
    // measures whether it is a plausible replacement for the tiny-skia export fallback.
    let mut cpu = RenderContext::new(view.width as u16, view.height as u16);
    cpu.set_paint(peniko::Color::new([clear[0], clear[1], clear[2], 1.0]));
    cpu.fill_rect(&vello_cpu::kurbo::Rect::new(0.0, 0.0, f64::from(view.width), f64::from(view.height)));
    let node = doc.node(id).ok_or("vector missing")?;
    let (path, _) = draw::node_outline(&doc, node).ok_or("vector outline missing")?;
    let xf = doc.world_transform(id);
    let shadow = node
        .effects
        .iter()
        .find_map(|effect| match &effect.kind {
            EffectKind::DropShadow(shadow) => Some(shadow),
            _ => None,
        })
        .ok_or("shadow missing")?;
    cpu.set_transform(xf * Affine::translate(shadow.offset));
    cpu.set_paint(peniko::Color::new([1.0; 4]));
    cpu.push_filter_layer(Filter::from_primitive(FilterPrimitive::DropShadowOnly {
        dx: 0.0,
        dy: 0.0,
        std_deviation: (shadow.radius / 2.0) as f32,
        color: peniko::Color::new([shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a]),
        edge_mode: EdgeMode::None,
    }));
    cpu.fill_path(&path);
    if spread > 0.0 {
        cpu.set_stroke(vello_cpu::kurbo::Stroke::new(spread * 2.0));
        cpu.stroke_path(&path);
    }
    cpu.pop_layer();
    cpu.set_transform(xf);
    cpu.set_paint(peniko::Color::new([0.8, 0.8, 0.85, fill_alpha]));
    cpu.fill_path(&path);
    cpu.flush();
    let mut resources = Resources::new();
    let mut pixels = Pixmap::new(view.width as u16, view.height as u16);
    cpu.render(&mut pixels, &mut resources);
    let (mean, changed, maximum) = difference(pixels.data_as_u8_slice(), reference.data(), view.width, view.height);
    println!("vello_cpu mean_rgb_error={mean:.3}/255 changed_gt8={changed:.3}% max={maximum}");
    Ok(())
}
