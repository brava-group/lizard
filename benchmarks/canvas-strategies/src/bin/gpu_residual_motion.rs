//! Offscreen 5120×2160 pan/zoom trace using one sharp overscan texture and soft effects.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::gpu_residual::GpuResidual;
use canvas_strategies_bench::residual::Residual;
use lizard_doc::EffectKind;
use lizard_render::{Options, View, render_view};

const WIDTH: u32 = 5120;
const HEIGHT: u32 = 2160;
const SOURCE_SCALE: f64 = 2.32;
const FACTOR: u32 = 5;

#[derive(Clone, Copy)]
struct Camera {
    center: (f64, f64),
    zoom: f64,
}

impl Camera {
    fn view(self) -> View {
        View {
            origin: (self.center.0 - f64::from(WIDTH) / self.zoom / 2.0, self.center.1 - f64::from(HEIGHT) / self.zoom / 2.0),
            scale: self.zoom,
            width: WIDTH,
            height: HEIGHT,
        }
    }
}

fn trace() -> Vec<Camera> {
    let mut seed = 0x91e1_0da5_83f4_22f1_u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / ((1_u64 << 53) as f64)
    };
    let mut result = Vec::with_capacity(120);
    let mut center = (1600.0, 1150.0);
    let mut velocity = (0.0, 0.0);
    for _ in 0..60 {
        velocity.0 = (velocity.0 * 0.72 + (random() - 0.5) * 110.0).clamp(-120.0, 120.0);
        velocity.1 = (velocity.1 * 0.72 + (random() - 0.5) * 75.0).clamp(-80.0, 80.0);
        center.0 = (center.0 + velocity.0).clamp(700.0, 2500.0);
        center.1 = (center.1 + velocity.1).clamp(600.0, 1700.0);
        result.push(Camera { center, zoom: 2.0 });
    }
    let mut zoom = 2.0;
    let mut direction = 1.0;
    for i in 0..60 {
        if i % 8 == 0 {
            direction = -direction;
        }
        zoom = (zoom + direction * (0.012 + random() * 0.026)).clamp(2.0, SOURCE_SCALE);
        result.push(Camera { center, zoom });
    }
    result
}

fn source_view(cameras: &[Camera]) -> Result<View, Box<dyn Error>> {
    if cameras.is_empty() {
        return Err("trace is empty".into());
    }
    let mut bounds = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for camera in cameras {
        let view = camera.view();
        bounds.0 = bounds.0.min(view.origin.0);
        bounds.1 = bounds.1.min(view.origin.1);
        bounds.2 = bounds.2.max(view.origin.0 + f64::from(WIDTH) / view.scale);
        bounds.3 = bounds.3.max(view.origin.1 + f64::from(HEIGHT) / view.scale);
    }
    let guard = 96.0;
    let source_scale = cameras.iter().map(|camera| camera.zoom).fold(0.0_f64, f64::max) + 0.02;
    let origin = (bounds.0 - guard, bounds.1 - guard);
    let width = (((bounds.2 + guard - origin.0) * source_scale).ceil() as u32).next_multiple_of(FACTOR);
    let height = (((bounds.3 + guard - origin.1) * source_scale).ceil() as u32).next_multiple_of(FACTOR);
    Ok(View { origin, scale: source_scale, width, height })
}

fn changed(actual: &[u8], reference: &[u8], threshold: u8) -> f64 {
    actual
        .chunks_exact(4)
        .zip(reference.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > threshold))
        .count() as f64
        * 100.0
        / f64::from(WIDTH * HEIGHT)
}

fn main() -> Result<(), Box<dyn Error>> {
    let cameras = trace();
    let source = source_view(&cameras)?;
    let low = View { origin: source.origin, scale: source.scale / f64::from(FACTOR), width: source.width / FACTOR, height: source.height / FACTOR };
    let (doc, warnings) = lizard_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let suppressed: Vec<_> = doc
        .nodes()
        .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
        .map(|node| node.id)
        .collect();
    let base_opts = Options { lod: true, skip_effects: true, skip: suppressed, ..Default::default() };
    println!(
        "source={}x{} low={}x{} output={}x{} source_scale={} cameras={} texture_mib={:.1}",
        source.width,
        source.height,
        low.width,
        low.height,
        WIDTH,
        HEIGHT,
        source.scale,
        cameras.len(),
        f64::from((source.width * source.height + low.width * low.height * 2 + WIDTH * HEIGHT) * 4) / 1_048_576.0
    );
    let start = Instant::now();
    let low_full = render_view(&doc, page, &low, &Options { lod: true, ..Default::default() }).ok_or("low full render failed")?;
    let low_full_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let low_base = render_view(&doc, page, &low, &base_opts).ok_or("low base render failed")?;
    let low_base_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let mut high_base = render_view(&doc, page, &source, &base_opts).ok_or("sharp overscan render failed")?;
    let high_base_ms = start.elapsed().as_secs_f64() * 1000.0;
    let baked = std::env::args().any(|arg| arg == "--baked");
    let start = Instant::now();
    if baked {
        let residual = Residual::new(low_full.clone(), low_base.clone(), low.clone()).ok_or("low views differ")?;
        residual.apply(&mut high_base, &source);
    }
    let bake_ms = if baked { start.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
    println!("prepare_cpu_ms low_full={low_full_ms:.2} low_base={low_base_ms:.2} sharp_overscan={high_base_ms:.2} bake={bake_ms:.2} baked={baked}");
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    if source.width.div_ceil(2) + 2 > device.limits().max_texture_dimension_2d || source.height > device.limits().max_texture_dimension_2d {
        return Err("overscan texture exceeds device limit".into());
    }
    let start = Instant::now();
    let compositor = GpuResidual::new(device, queue, source.width, source.height, low.width, low.height, WIDTH, HEIGHT, baked, None);
    let setup_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    compositor.upload_images(&high_base, &low_full, &low_base)?;
    let upload_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!("gpu_setup_ms={setup_ms:.2} upload_ms={upload_ms:.2}");
    let mut submit = Vec::with_capacity(cameras.len());
    let mut complete = Vec::with_capacity(cameras.len());
    for (index, camera) in cameras.iter().enumerate() {
        let view = camera.view();
        let start = Instant::now();
        compositor.render_submit(&source, &view);
        submit.push(start.elapsed().as_secs_f64() * 1000.0);
        compositor.wait()?;
        complete.push(start.elapsed().as_secs_f64() * 1000.0);
        if [0, 30, 59, 60, 90, 119].contains(&index) {
            let actual = compositor.pixels()?;
            let reference = render_view(&doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("reference view failed")?;
            println!(
                "quality frame={index} zoom={:.3} changed_4={:.3}% changed_16={:.3}%",
                camera.zoom,
                changed(&actual, reference.data(), 4),
                changed(&actual, reference.data(), 16)
            );
        }
    }
    submit.sort_by(f64::total_cmp);
    complete.sort_by(f64::total_cmp);
    println!(
        "motion submit_median_ms={:.2} submit_p95_ms={:.2} complete_median_ms={:.2} complete_p95_ms={:.2} complete_max_ms={:.2} over_50ms={}/{}",
        submit[submit.len() / 2],
        submit[(submit.len() * 95 / 100).min(submit.len() - 1)],
        complete[complete.len() / 2],
        complete[(complete.len() * 95 / 100).min(complete.len() - 1)],
        complete[complete.len() - 1],
        complete.iter().filter(|&&value| value > 50.0).count(),
        complete.len(),
    );
    Ok(())
}
