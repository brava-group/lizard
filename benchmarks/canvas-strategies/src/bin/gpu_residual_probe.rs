//! Measure sharp geometry plus low-resolution effects composed in one Metal pass.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::gpu_residual::GpuResidual;
use lizard_doc::EffectKind;
use lizard_render::{Options, View, render_view};

fn main() -> Result<(), Box<dyn Error>> {
    let icon = std::env::args().any(|arg| arg == "--icon");
    let geometry_only = std::env::args().any(|arg| arg == "--geometry-only");
    let path = if icon {
        Path::new("/Users/matheuspires/Documents/Lizard-new-icon.lizard")
    } else {
        Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard")
    };
    let (doc, warnings) = lizard_format::load_file(path)?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let (width, height, default_factor, view) = if icon {
        let frame = *doc.top_frames(page).first().ok_or("icon frame missing")?;
        let bounds = doc.world_render_bounds(frame).ok_or("icon frame bounds missing")?;
        let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let scale = (1024.0 / bounds.width().max(bounds.height()).max(1.0)).clamp(0.25, 4.0);
        (1024, 1024, 4, View { origin: (center.0 - 512.0 / scale, center.1 - 512.0 / scale), scale, width: 1024, height: 1024 })
    } else {
        (5120, 2160, 5, View { origin: (320.0, 610.0), scale: 2.0, width: 5120, height: 2160 })
    };
    let factor = std::env::args().find_map(|arg| arg.strip_prefix("--factor=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(default_factor);
    if !(2..=8).contains(&factor) || width % factor != 0 || height % factor != 0 {
        return Err("factor must be 2..=8 and divide both image dimensions".into());
    }
    let source = if std::env::args().any(|arg| arg == "--extended") {
        let guard = 96.0;
        let scale = view.scale + 0.02;
        View {
            origin: (view.origin.0 - guard, view.origin.1 - guard),
            scale,
            width: (((f64::from(width) / view.scale + guard * 2.0) * scale).ceil() as u32).next_multiple_of(factor),
            height: (((f64::from(height) / view.scale + guard * 2.0) * scale).ceil() as u32).next_multiple_of(factor),
        }
    } else {
        view.clone()
    };
    let low_view =
        View { origin: source.origin, scale: source.scale / f64::from(factor), width: source.width / factor, height: source.height / factor };
    let suppressed: Vec<_> = doc
        .nodes()
        .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
        .map(|node| node.id)
        .collect();
    let base_opts = Options { lod: true, skip_effects: true, skip: suppressed, ..Default::default() };
    let started = Instant::now();
    let low_full = render_view(&doc, page, &low_view, &Options { lod: true, ..Default::default() }).ok_or("low full render failed")?;
    let low_full_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let low_base = render_view(&doc, page, &low_view, &base_opts).ok_or("low base render failed")?;
    let low_base_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let high_base = render_view(&doc, page, &source, &base_opts).ok_or("high base render failed")?;
    let high_base_ms = started.elapsed().as_secs_f64() * 1000.0;
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let setup = Instant::now();
    let compositor =
        GpuResidual::new(device, queue, source.width, source.height, low_view.width, low_view.height, width, height, geometry_only, None);
    let pipeline_setup_ms = setup.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    compositor.upload_images(&high_base, &low_full, &low_base)?;
    let upload_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    compositor.render_submit(&source, &view);
    compositor.wait()?;
    let gpu_compose_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let actual = compositor.pixels()?;
    let readback_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let full_opts = Options { lod: true, ..Default::default() };
    let reference = render_view(&doc, page, &view, if geometry_only { &base_opts } else { &full_opts }).ok_or("reference failed")?;
    let reference_ms = started.elapsed().as_secs_f64() * 1000.0;
    let total = f64::from(width * height);
    let changed = |threshold: u8| {
        actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > threshold))
            .count() as f64
            * 100.0
            / total
    };
    println!("document={} size={}x{} factor={factor} adapter={}", path.display(), width, height, adapter.get_info().name);
    println!(
        "low_full_ms={low_full_ms:.2} low_base_ms={low_base_ms:.2} high_base_ms={high_base_ms:.2} pipeline_setup_ms={pipeline_setup_ms:.2} upload_ms={upload_ms:.2} gpu_compose_ms={gpu_compose_ms:.2} readback_ms={readback_ms:.2} reference_ms={reference_ms:.2}"
    );
    println!("changed_4={:.3}% changed_16={:.3}%", changed(4), changed(16));
    if std::env::args().any(|arg| arg == "--save") {
        let output = if icon { "target/canvas-strategies/gpu-residual-icon.png" } else { "target/canvas-strategies/gpu-residual-workbook.png" };
        let file = std::fs::File::create(output)?;
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&actual)?;
        let reference_file = std::fs::File::create("target/canvas-strategies/gpu-residual-reference.png")?;
        let mut reference_encoder = png::Encoder::new(reference_file, width, height);
        reference_encoder.set_color(png::ColorType::Rgba);
        reference_encoder.set_depth(png::BitDepth::Eight);
        reference_encoder.write_header()?.write_image_data(reference.data())?;
    }
    Ok(())
}
