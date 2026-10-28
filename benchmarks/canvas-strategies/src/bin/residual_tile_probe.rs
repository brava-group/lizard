//! Measure a provisional sharp/effect-faithful tile path at realistic viewport sizes.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::residual::Residual;
use figcraft_doc::EffectKind;
use figcraft_render::{Options, Pixmap, View, render_view};
use rayon::prelude::*;

const WIDTH: u32 = 5120;
const HEIGHT: u32 = 2160;
const TILE: u32 = 512;

fn compare(actual: &Pixmap, reference: &Pixmap) -> (f64, f64) {
    let total = f64::from(actual.width() * actual.height());
    let changed = |threshold: u8| {
        actual
            .data()
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > threshold))
            .count() as f64
            * 100.0
            / total
    };
    (changed(4), changed(16))
}

fn main() -> Result<(), Box<dyn Error>> {
    let (doc, warnings) = figcraft_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let suppressed: Vec<_> = doc
        .nodes()
        .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
        .map(|node| node.id)
        .collect();
    let factor = std::env::args().find_map(|arg| arg.strip_prefix("--factor=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(5).clamp(2, 8);
    let workers =
        std::env::args().find_map(|arg| arg.strip_prefix("--workers=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(4).clamp(1, 8);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(workers).build()?;
    println!("factor={factor} workers={workers} suppressed_layer_blur_nodes={}", suppressed.len());
    let cameras = [("initial", (1600.0, 1150.0), 2.0), ("pan", (2300.0, 1500.0), 2.0), ("zoom", (2300.0, 1500.0), 2.25)];
    for (label, center, scale) in cameras {
        let view = View {
            origin: (center.0 - f64::from(WIDTH) / scale / 2.0, center.1 - f64::from(HEIGHT) / scale / 2.0),
            scale,
            width: WIDTH,
            height: HEIGHT,
        };
        let low = View { origin: view.origin, scale: view.scale / f64::from(factor), width: WIDTH.div_ceil(factor), height: HEIGHT.div_ceil(factor) };
        let started = Instant::now();
        let full = render_view(&doc, page, &low, &Options { lod: true, ..Default::default() }).ok_or("low effect render failed")?;
        let low_full_ms = started.elapsed().as_secs_f64() * 1000.0;
        let base_options = Options { lod: true, skip_effects: true, skip: suppressed.clone(), ..Default::default() };
        let started = Instant::now();
        let base = render_view(&doc, page, &low, &base_options).ok_or("low base render failed")?;
        let low_base_ms = started.elapsed().as_secs_f64() * 1000.0;
        let residual = Residual::new(full, base, low).ok_or("residual views differ")?;
        let tiles: Vec<_> = (0..HEIGHT.div_ceil(TILE)).flat_map(|row| (0..WIDTH.div_ceil(TILE)).map(move |column| (column, row))).collect();
        let started = Instant::now();
        let rendered: Vec<_> = pool.install(|| {
            tiles
                .par_iter()
                .map(|&(column, row)| {
                    let tile_view = View {
                        origin: (view.origin.0 + f64::from(column * TILE) / view.scale, view.origin.1 + f64::from(row * TILE) / view.scale),
                        scale: view.scale,
                        width: TILE.min(WIDTH - column * TILE),
                        height: TILE.min(HEIGHT - row * TILE),
                    };
                    let mut image = render_view(&doc, page, &tile_view, &base_options).ok_or_else(|| "sharp tile failed".to_owned())?;
                    residual.apply(&mut image, &tile_view);
                    Ok::<_, String>((column, row, image))
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        let tiles_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut output = Pixmap::new(WIDTH, HEIGHT).ok_or("output allocation failed")?;
        for (column, row, image) in rendered {
            let width = image.width() as usize;
            for y in 0..image.height() as usize {
                let from = y * width * 4;
                let to = ((row as usize * TILE as usize + y) * WIDTH as usize + column as usize * TILE as usize) * 4;
                output.data_mut()[to..to + width * 4].copy_from_slice(&image.data()[from..from + width * 4]);
            }
        }
        let started = Instant::now();
        let reference = render_view(&doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("faithful view failed")?;
        let reference_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (changed4, changed16) = compare(&output, &reference);
        println!(
            "view={label} low_full_ms={low_full_ms:.2} low_base_ms={low_base_ms:.2} tiles_ms={tiles_ms:.2} provisional_total_ms={:.2} reference_ms={reference_ms:.2} changed_4={changed4:.3}% changed_16={changed16:.3}%",
            low_full_ms + low_base_ms + tiles_ms
        );
    }
    Ok(())
}
