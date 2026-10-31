//! Compare the cached GPU progressive layer blur with the document renderer.

use std::error::Error;
use std::time::Instant;

use lizard_canvas_gpu::vello_scene::{GpuCanvas, has_unhandled_effects_with_cached_noise_in_view};
use lizard_color::{Color, Paint};
use lizard_doc::{Blur, BlurType, Document, Effect, EffectKind, NodeKind};
use lizard_render::{Options, View, render_view};

fn difference(actual: &[u8], reference: &[u8]) -> (f64, f64, u8) {
    let mut total = 0u64;
    let mut changed = 0usize;
    let mut maximum = 0u8;
    let mut pixels = 0usize;
    for (a, b) in actual.chunks_exact(4).zip(reference.chunks_exact(4)) {
        let delta = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
        total += delta.iter().map(|value| u64::from(*value)).sum::<u64>();
        changed += usize::from(delta.iter().any(|value| *value > 8));
        maximum = maximum.max(*delta.iter().max().unwrap_or(&0));
        pixels += 1;
    }
    (total as f64 / (pixels * 3).max(1) as f64, 100.0 * changed as f64 / pixels.max(1) as f64, maximum)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut doc = Document::new("GPU progressive blur parity");
    let page = doc.current_page.ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Rectangle, "Gradient blur", 55.0, 45.0, 110.0, 100.0)?;
    let node = doc.node_mut(id).ok_or("missing rectangle")?;
    node.fills = vec![Paint::solid(Color::new(0.25, 0.15, 0.8, 1.0))];
    node.effects = vec![Effect {
        kind: EffectKind::LayerBlur(Blur { radius: 16.0, blur_type: BlurType::Progressive, start_radius: 0.0, start: (0.0, 0.0), end: (1.0, 1.0) }),
        visible: true,
    }];
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, background_visible: true, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    for scale in [1.0, 2.0] {
        let view = View { origin: (0.0, 0.0), scale, width: 220 * scale as u32, height: 200 * scale as u32 };
        let mut gpu = GpuCanvas::new(view.width, view.height)?;
        let start = Instant::now();
        let prepared = gpu.prepare_static_effects(&doc)?;
        gpu.wait()?;
        let prepare_ms = start.elapsed().as_secs_f64() * 1000.0;
        let supported = !has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true);
        gpu.render_submit_document(&doc, page, &view, clear)?;
        gpu.wait()?;
        let actual = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU render failed")?;
        let (mean, changed, max) = difference(&actual, reference.data());
        println!(
            "scale={scale} prepared_blurs={} supported={supported} prepare_ms={prepare_ms:.2} mean_rgb_error={mean:.3}/255 changed_gt8={changed:.3}% max={max}",
            prepared.0
        );
    }
    Ok(())
}
