//! Time a complete viewport vector render as a possible sharp startup fallback.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view};
use figcraft_doc::NodeKind;
use figcraft_render::View;

fn main() -> Result<(), Box<dyn Error>> {
    let (doc, warnings) = figcraft_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let view = View { origin: (320.0, 610.0), scale: 2.0, width: 2560, height: 2160 };
    let started = Instant::now();
    let mut gpu = GpuCanvas::new(view.width, view.height)?;
    gpu.register_images(&doc)?;
    let index = BackdropIndex::new(&doc, page);
    println!("setup_ms={:.2} fallback_needed={}", started.elapsed().as_secs_f64() * 1000.0, has_unhandled_effects_in_view(&doc, &view));
    for i in 0..4 {
        let view = View { origin: (320.0 + f64::from(i % 2) * 1280.0, 610.0), ..view.clone() };
        let started = Instant::now();
        let (render_ms, passes) = gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, clear, &index)?;
        println!("frame={i} submit_ms={:.2} render_ms={render_ms:.2} backdrop_passes={passes}", started.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(())
}
