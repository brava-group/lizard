//! Compare the current native GPU effect path with the saved Lizard icon document.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use figcraft_canvas_gpu::vello_scene::{GpuCanvas, has_unhandled_effects_with_cached_noise_in_view};
use figcraft_doc::NodeKind;
use figcraft_render::{Options, View, render_view};

fn main() -> Result<(), Box<dyn Error>> {
    let path = Path::new("/Users/matheuspires/Documents/Lizard-new-icon.figcraft");
    let (doc, warnings) = figcraft_format::load_file(path)?;
    if !warnings.is_empty() {
        return Err(format!("document warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let frame = *doc.top_frames(page).first().ok_or("icon frame missing")?;
    let bounds = doc.world_render_bounds(frame).ok_or("icon bounds missing")?;
    let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    let clear = doc
        .node(page)
        .and_then(|node| match &node.kind {
            NodeKind::Page { background, .. } => Some([background.r, background.g, background.b]),
            _ => None,
        })
        .ok_or("page background missing")?;
    for scale in [1.0, 2.0] {
        let side = (1024.0 * scale) as u32;
        let view = View {
            origin: (center.0 - f64::from(side) / (2.0 * scale), center.1 - f64::from(side) / (2.0 * scale)),
            scale,
            width: side,
            height: side,
        };
        let supported = !has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true);
        let started = Instant::now();
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("reference render failed")?;
        let reference_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut gpu = GpuCanvas::new(side, side)?;
        let started = Instant::now();
        let caches = gpu.prepare_static_effects(&doc)?;
        let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        gpu.render_submit_document(&doc, page, &view, clear)?;
        let first_ms = started.elapsed().as_secs_f64() * 1000.0;
        let actual = gpu.pixels()?;
        if scale == 1.0 && std::env::var_os("LIZARD_BENCH_WRITE_PNG").is_some() {
            for (suffix, pixels) in [("reference", reference.data()), ("gpu", actual.as_slice())] {
                let output = std::fs::File::create(format!("/tmp/lizard-icon-{suffix}.png"))?;
                let mut encoder = png::Encoder::new(output, side, side);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header()?.write_image_data(pixels)?;
            }
        }
        let mut error = 0u64;
        let mut changed = 0usize;
        for (a, b) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)) {
            let delta = [a[0].abs_diff(b[0]), a[1].abs_diff(b[1]), a[2].abs_diff(b[2])];
            changed += usize::from(delta.iter().any(|v| *v > 8));
            error += delta.iter().map(|v| u64::from(*v)).sum::<u64>();
        }
        let started = Instant::now();
        gpu.render_submit_document(&doc, page, &view, clear)?;
        let warm_ms = started.elapsed().as_secs_f64() * 1000.0;
        println!(
            "scale={scale} supported={supported} caches={caches:?} reference_ms={reference_ms:.2} prepare_ms={prepare_ms:.2} first_ms={first_ms:.2} warm_ms={warm_ms:.2} mean_rgb={:.3}/255 changed_gt8={:.3}%",
            error as f64 / (f64::from(side * side) * 3.0),
            changed as f64 * 100.0 / f64::from(side * side),
        );
    }
    Ok(())
}
