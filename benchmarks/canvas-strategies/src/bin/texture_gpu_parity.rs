//! Compare isolated GPU texture scatter against the existing document renderer.

use std::error::Error;
use std::time::Instant;

use lizard_canvas_gpu::vello_scene::GpuCanvas;
use lizard_color::{Color, Paint};
use lizard_doc::{Document, Effect, EffectKind, NodeKind, Texture, VectorNetwork};
use lizard_geom::{Affine, Point};
use lizard_render::{Options, View, render_view};

fn run(clip_to_shape: bool, scale: f64) -> Result<(), Box<dyn Error>> {
    let large = std::env::var_os("LIZARD_NOISE_LARGE").is_some();
    let mut doc = Document::new("GPU texture parity");
    let page = doc.current_page.ok_or("new document has no page")?;
    let (x, y, width, height) = if large { (100.0, 100.0, 1200.0, 900.0) } else { (25.0, 22.0, 118.0, 97.0) };
    let shape = if std::env::var_os("LIZARD_NOISE_VECTOR").is_some() {
        let network = VectorNetwork::from_points(
            &[
                Point::new(0.0, 0.0),
                Point::new(width, 0.0),
                Point::new(width * 0.8, height * 0.8),
                Point::new(width * 0.3, height),
                Point::new(0.0, height * 0.7),
            ],
            true,
        );
        NodeKind::Vector { network }
    } else {
        NodeKind::Rectangle
    };
    let id = doc.create(page, shape, "Textured shape", x, y, width, height)?;
    let node = doc.node_mut(id).ok_or("shape missing")?;
    if std::env::var_os("LIZARD_NOISE_ROTATED").is_some() {
        node.transform = Affine::translate((x, y)) * Affine::rotate(std::f64::consts::FRAC_PI_6);
    }
    let alpha = std::env::var("LIZARD_NOISE_ALPHA").ok().and_then(|text| text.parse().ok()).unwrap_or(1.0);
    node.fills = vec![Paint::solid(Color::new(0.8, 0.7, 0.6, alpha))];
    let texture = Texture { size: 2.0, size_y: None, radius: 3.0, clip_to_shape };
    node.effects = vec![Effect { kind: EffectKind::Texture(texture), visible: true }];
    let view = View { origin: (0.0, 0.0), scale, width: if large { 2048 } else { 256 }, height: if large { 1536 } else { 256 } };
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, background_visible: true, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let cpu_start = Instant::now();
    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU render failed")?;
    let cpu_ms = cpu_start.elapsed().as_secs_f64() * 1000.0;
    let mut gpu = GpuCanvas::new(view.width, view.height)?;
    let prepare_start = Instant::now();
    gpu.prepare_static_effects(&doc)?;
    let prepare_ms = prepare_start.elapsed().as_secs_f64() * 1000.0;
    if gpu.cached_texture_count() != 1 {
        return Err("GPU texture scatter was not prepared".into());
    }
    let reuse_start = Instant::now();
    gpu.prepare_static_effects(&doc)?;
    let reuse_ms = reuse_start.elapsed().as_secs_f64() * 1000.0;
    let frame_start = Instant::now();
    let submit_ms = gpu.render_submit_document(&doc, page, &view, clear)?;
    gpu.wait()?;
    let fence_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
    let actual = gpu.pixels()?;
    let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
    let mut error = 0u64;
    let mut changed = 0usize;
    let mut max = 0u8;
    for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
        let mut pixel_changed = false;
        for channel in 0..4 {
            let delta = a[channel].abs_diff(b[channel]);
            error += u64::from(delta);
            max = max.max(delta);
            pixel_changed |= delta > 8;
        }
        changed += usize::from(pixel_changed);
    }
    let pixels = f64::from(view.width * view.height);
    println!(
        "clip={clip_to_shape} scale={scale:.1} mean_rgba_error={:.3}/255 pixels_gt8={:.3}% max={max} cpu_ms={cpu_ms:.2} prepare_ms={prepare_ms:.2} reuse_ms={reuse_ms:.2} submit_ms={submit_ms:.2} fence_ms={fence_ms:.2} readback_ms={:.2}",
        error as f64 / (pixels * 4.0),
        changed as f64 * 100.0 / pixels,
        frame_ms - fence_ms
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    for clip_to_shape in [false, true] {
        for scale in [1.0, 2.0] {
            run(clip_to_shape, scale)?;
        }
    }
    Ok(())
}
