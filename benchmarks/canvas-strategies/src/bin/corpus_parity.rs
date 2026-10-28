//! Compare the GPU scene with the document renderer across saved documents and view scales.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Instant;

use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view};
use figcraft_doc::{Document, NodeId, NodeKind};
use figcraft_render::{Options, View, render_view};

const SIDE: u32 = 512;
const LIMIT: usize = 16;

fn clear_color(doc: &Document, page: NodeId) -> [f32; 3] {
    match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, background_visible: true, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    }
}

fn views(doc: &Document, page: NodeId) -> Vec<(String, View)> {
    let frames = doc.top_frames(page);
    let selected: Vec<_> = if frames.is_empty() {
        Vec::new()
    } else {
        let count = frames.len().min(LIMIT);
        (0..count).map(|index| frames[index * frames.len() / count]).collect()
    };
    let mut result = Vec::new();
    for (index, id) in selected.into_iter().enumerate() {
        let Some(bounds) = doc.world_render_bounds(id) else { continue };
        if ![bounds.x0, bounds.y0, bounds.x1, bounds.y1].iter().all(|value| value.is_finite()) {
            continue;
        }
        let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let fit = (f64::from(SIDE) / bounds.width().max(bounds.height()).max(1.0)).clamp(0.25, 4.0);
        for (name, scale) in [("fit", fit), ("detail", (fit * 2.0).clamp(0.5, 8.0))] {
            result.push((
                format!("frame{index}-{name}"),
                View {
                    origin: (center.0 - f64::from(SIDE) / (2.0 * scale), center.1 - f64::from(SIDE) / (2.0 * scale)),
                    scale,
                    width: SIDE,
                    height: SIDE,
                },
            ));
        }
    }
    if result.is_empty()
        && let Some(bounds) = doc.page_content_bounds(page)
    {
        let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let scale = (f64::from(SIDE) / bounds.width().max(bounds.height()).max(1.0)).clamp(0.25, 4.0);
        result.push((
            "page-fit".to_string(),
            View {
                origin: (center.0 - f64::from(SIDE) / (2.0 * scale), center.1 - f64::from(SIDE) / (2.0 * scale)),
                scale,
                width: SIDE,
                height: SIDE,
            },
        ));
    }
    result
}

fn difference(actual: &[u8], reference: &[u8]) -> (f64, f64, f64, f64) {
    let mut changed = [0usize; 3];
    let mut error = 0u64;
    for (actual, reference) in actual.chunks_exact(4).zip(reference.chunks_exact(4)) {
        let delta: [u8; 4] = std::array::from_fn(|index| actual[index].abs_diff(reference[index]));
        for (index, threshold) in [4, 16, 32].into_iter().enumerate() {
            changed[index] += usize::from(delta.iter().any(|value| *value > threshold));
        }
        error += delta.iter().map(|value| u64::from(*value)).sum::<u64>();
    }
    let percentage = |count: usize| count as f64 * 100.0 / f64::from(SIDE * SIDE);
    (percentage(changed[0]), percentage(changed[1]), percentage(changed[2]), error as f64 / f64::from(SIDE * SIDE * 4))
}

fn save_png(path: &Path, pixels: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), SIDE, SIDE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}

fn run(path: &Path, gpu: &mut GpuCanvas) -> Result<(), Box<dyn Error>> {
    let (doc, warnings) = figcraft_format::load_file(path)?;
    if !warnings.is_empty() {
        eprintln!("{}: {} load warnings", path.display(), warnings.len());
    }
    let page = doc.current_page.or_else(|| doc.first_page()).ok_or("document has no page")?;
    let cases = views(&doc, page);
    let images = gpu.register_images(&doc)?;
    let clear = clear_color(&doc, page);
    let backdrop = BackdropIndex::new(&doc, page);
    println!("document,path={},nodes={},images={},views={}", path.display(), doc.nodes().count(), images, cases.len());
    for (label, view) in cases {
        let fallback = has_unhandled_effects_in_view(&doc, &view);
        let cpu_start = Instant::now();
        let reference = render_view(&doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("CPU render failed")?;
        let cpu_ms = cpu_start.elapsed().as_secs_f64() * 1000.0;
        let gpu_start = Instant::now();
        let (_, backdrop_count) = gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, clear, &backdrop)?;
        let actual = gpu.pixels()?;
        let gpu_ms = gpu_start.elapsed().as_secs_f64() * 1000.0;
        let (changed, changed_16, changed_32, mean_abs) = difference(&actual, reference.data());
        if !fallback && changed >= 2.0 {
            let stem = path.file_stem().and_then(|part| part.to_str()).unwrap_or("document");
            let directory = Path::new("target/canvas-strategies/parity");
            std::fs::create_dir_all(directory)?;
            save_png(&directory.join(format!("{stem}-{label}-cpu.png")), reference.data())?;
            save_png(&directory.join(format!("{stem}-{label}-gpu.png")), &actual)?;
        }
        println!(
            "view,path={},label={},scale={:.4},fallback={},backdrops={},changed_pct={changed:.4},changed_16_pct={changed_16:.4},changed_32_pct={changed_32:.4},mean_abs={mean_abs:.4},cpu_ms={cpu_ms:.2},gpu_readback_ms={gpu_ms:.2}",
            path.display(),
            label,
            view.scale,
            fallback,
            backdrop_count
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let paths: Vec<PathBuf> = if std::env::args_os().len() > 1 {
        std::env::args_os().skip(1).map(PathBuf::from).collect()
    } else {
        [
            "examples/ridgeline-studio-dashboard.figcraft",
            "examples/ridgeline-design-system.figcraft",
            "examples/glass-and-vectors.figcraft",
            "examples/ridgeline-app.figcraft",
            "examples/ridgeline-onboarding.figcraft",
            "benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft",
            "/Users/matheuspires/Documents/Lizard-new-icon.figcraft",
        ]
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
    };
    let mut gpu = GpuCanvas::new(SIDE, SIDE)?;
    println!("adapter={},scene={}x{},threshold=4/255", gpu.adapter_name(), SIDE, SIDE);
    for path in paths {
        run(&path, &mut gpu)?;
    }
    Ok(())
}
