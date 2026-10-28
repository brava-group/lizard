//! Measure live full-viewport GPU vectors with document-wide cached soft effects.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::gpu_live_residual::GpuLiveResidual;
use canvas_strategies_bench::gpu_scene::{GpuCanvas, make_scene_with_images};
use figcraft_doc::NodeKind;
use figcraft_render::{Options, Pixmap, View, render_view};

const WIDTH: u32 = 5120;
const HEIGHT: u32 = 2160;

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("LIZARD_GPU_NO_EFFECTS").is_none() {
        return Err("set LIZARD_GPU_NO_EFFECTS=1 to benchmark geometry plus residual".into());
    }
    let (doc, warnings) = figcraft_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let root = doc.node(page).ok_or("page node missing")?;
    let mut bounds = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &id in &root.children {
        if let Some(rect) = doc.world_render_bounds(id) {
            bounds.0 = bounds.0.min(rect.x0);
            bounds.1 = bounds.1.min(rect.y0);
            bounds.2 = bounds.2.max(rect.x1);
            bounds.3 = bounds.3.max(rect.y1);
        }
    }
    if !bounds.0.is_finite() || !bounds.2.is_finite() {
        return Err("document render bounds missing".into());
    }
    let guard = 128.0;
    let low_scale = 2.4 / 5.0;
    let low = View {
        origin: (bounds.0 - guard, bounds.1 - guard),
        scale: low_scale,
        width: (((bounds.2 - bounds.0) + guard * 2.0) * low_scale).ceil() as u32,
        height: (((bounds.3 - bounds.1) + guard * 2.0) * low_scale).ceil() as u32,
    };
    println!("low_source={}x{} world_bounds={bounds:?}", low.width, low.height);
    let started = Instant::now();
    let low_full = render_view(&doc, page, &low, &Options { lod: true, ..Default::default() }).ok_or("low full render failed")?;
    let low_full_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let mut gpu = GpuCanvas::new(WIDTH, HEIGHT)?;
    let images = gpu.register_images(&doc)?;
    let mut low_gpu = GpuCanvas::shared_with(&gpu, low.width, low.height);
    low_gpu.register_images(&doc)?;
    let low_scene = make_scene_with_images(&doc, page, &low, Some(low_gpu.image_info()));
    low_gpu.render_submit_with_clear(&low_scene, clear)?;
    let low_pixels = low_gpu.pixels()?;
    let mut low_base = Pixmap::new(low.width, low.height).ok_or("low pixmap allocation failed")?;
    low_base.data_mut().copy_from_slice(&low_pixels);
    let low_base_ms = started.elapsed().as_secs_f64() * 1000.0;
    let residual = GpuLiveResidual::new(gpu.device(), gpu.queue(), gpu.output_view(), low.clone(), WIDTH, HEIGHT, None);
    residual.upload(&low_full, &low_base)?;
    println!("images={images} low_full_ms={low_full_ms:.2} low_base_gpu_ms={low_base_ms:.2}");
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
    let mut times = Vec::with_capacity(120);
    for i in 0..120 {
        if i < 60 {
            velocity.0 = (velocity.0 * 0.72 + (random() - 0.5) * 110.0).clamp(-120.0, 120.0);
            velocity.1 = (velocity.1 * 0.72 + (random() - 0.5) * 75.0).clamp(-80.0, 80.0);
            center.0 = (center.0 + velocity.0).clamp(700.0, 2500.0);
            center.1 = (center.1 + velocity.1).clamp(600.0, 1700.0);
        } else {
            zoom = (zoom + if i % 16 < 8 { 0.022 } else { -0.022 }).clamp(2.0, 2.32);
        }
        let view = View {
            origin: (center.0 - f64::from(WIDTH) / zoom / 2.0, center.1 - f64::from(HEIGHT) / zoom / 2.0),
            scale: zoom,
            width: WIDTH,
            height: HEIGHT,
        };
        let started = Instant::now();
        let scene = make_scene_with_images(&doc, page, &view, Some(gpu.image_info()));
        let scene_ms = started.elapsed().as_secs_f64() * 1000.0;
        gpu.render_submit_with_clear(&scene, clear)?;
        residual.render_submit(&view);
        residual.wait()?;
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        times.push(total_ms);
        if [0, 30, 59, 60, 90, 119].contains(&i) {
            let actual = residual.pixels()?;
            let reference = render_view(&doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("reference render failed")?;
            let changed = |threshold: u8| {
                actual
                    .chunks_exact(4)
                    .zip(reference.data().chunks_exact(4))
                    .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > threshold))
                    .count() as f64
                    * 100.0
                    / f64::from(WIDTH * HEIGHT)
            };
            println!(
                "frame={i} zoom={zoom:.3} scene_ms={scene_ms:.2} total_ms={total_ms:.2} changed_4={:.3}% changed_16={:.3}%",
                changed(4),
                changed(16)
            );
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
