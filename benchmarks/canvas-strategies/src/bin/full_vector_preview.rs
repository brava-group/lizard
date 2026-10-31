//! Time a complete viewport vector render as a possible sharp startup fallback.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view, make_scene_with_images};
use lizard_doc::NodeKind;
use lizard_render::View;

fn main() -> Result<(), Box<dyn Error>> {
    let (doc, warnings) = lizard_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let geometry_only = std::env::args().any(|arg| arg == "--geometry-only");
    let view = View { origin: (320.0, 610.0), scale: 2.0, width: if geometry_only { 5120 } else { 2560 }, height: 2160 };
    let started = Instant::now();
    let mut gpu = GpuCanvas::new(view.width, view.height)?;
    gpu.register_images(&doc)?;
    let index = BackdropIndex::new(&doc, page);
    println!("setup_ms={:.2} fallback_needed={}", started.elapsed().as_secs_f64() * 1000.0, has_unhandled_effects_in_view(&doc, &view));
    let motion = std::env::args().any(|arg| arg == "--motion");
    let mut times = Vec::new();
    let mut seed = 0x91e1_0da5_83f4_22f1_u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / ((1_u64 << 53) as f64)
    };
    let mut center = (1600.0, 1150.0);
    let mut velocity = (0.0, 0.0);
    let mut zoom = 2.0_f64;
    for i in 0..if motion { 120 } else { 4 } {
        if motion {
            if i < 60 {
                velocity.0 = (velocity.0 * 0.72 + (random() - 0.5) * 110.0).clamp(-120.0, 120.0);
                velocity.1 = (velocity.1 * 0.72 + (random() - 0.5) * 75.0).clamp(-80.0, 80.0);
                center.0 = (center.0 + velocity.0).clamp(700.0, 2500.0);
                center.1 = (center.1 + velocity.1).clamp(600.0, 1700.0);
            } else {
                zoom = (zoom + if i % 16 < 8 { 0.022 } else { -0.022 }).clamp(2.0, 2.32);
            }
        } else {
            center.0 = 1600.0 + f64::from(i % 2) * 1280.0;
        }
        let view = View {
            origin: (center.0 - f64::from(view.width) / zoom / 2.0, center.1 - f64::from(view.height) / zoom / 2.0),
            scale: zoom,
            ..view.clone()
        };
        let started = Instant::now();
        let (render_ms, passes) = if geometry_only {
            let scene = make_scene_with_images(&doc, page, &view, Some(gpu.image_info()));
            let submit = gpu.render_submit_with_clear(&scene, clear)?;
            gpu.wait()?;
            (submit, 0)
        } else {
            gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, clear, &index)?
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        times.push(ms);
        if !motion || [0, 30, 59, 60, 90, 119].contains(&i) {
            println!("frame={i} total_ms={ms:.2} render_ms={render_ms:.2} backdrop_passes={passes}");
        }
    }
    times.sort_by(f64::total_cmp);
    println!(
        "motion frames={} median_ms={:.2} p95_ms={:.2} max_ms={:.2} over_50ms={}",
        times.len(),
        times[times.len() / 2],
        times[(times.len() * 95 / 100).min(times.len() - 1)],
        times[times.len() - 1],
        times.iter().filter(|&&ms| ms > 50.0).count()
    );
    Ok(())
}
