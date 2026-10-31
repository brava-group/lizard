//! Check whether a CPU-rendered region can be cropped into faithful retained tiles.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use lizard_render::{Options, View, render_view};

const TILE: u32 = 512;
const GRID: u32 = 4;
const SCALE: f64 = 2.0;

fn main() -> Result<(), Box<dyn Error>> {
    let (doc, warnings) = lizard_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let bleed = (doc.max_backdrop_radius() * 2.32 * 1.5 + 3.0).ceil().clamp(3.0, 256.0) as u32;
    let side = TILE + bleed * 2;
    let first = (4_i32, 3_i32);
    let region = View {
        origin: ((f64::from(first.0 * TILE as i32) - f64::from(bleed)) / SCALE, (f64::from(first.1 * TILE as i32) - f64::from(bleed)) / SCALE),
        scale: SCALE,
        width: TILE * GRID + bleed * 2,
        height: TILE * GRID + bleed * 2,
    };
    let opts = Options { lod: true, ..Default::default() };
    let started = Instant::now();
    let image = render_view(&doc, page, &region, &opts).ok_or("region render failed")?;
    println!("region={}x{} ms={:.2} bleed={bleed}", region.width, region.height, started.elapsed().as_secs_f64() * 1000.0);
    for y in 0..GRID {
        for x in 0..GRID {
            let tile = View {
                origin: (region.origin.0 + f64::from(x * TILE) / SCALE, region.origin.1 + f64::from(y * TILE) / SCALE),
                scale: SCALE,
                width: side,
                height: side,
            };
            let started = Instant::now();
            let reference = render_view(&doc, page, &tile, &opts).ok_or("tile render failed")?;
            let mut changed_full = 0usize;
            let mut changed_center = 0usize;
            for ty in 0..side {
                for tx in 0..side {
                    let from = (((y * TILE + ty) * region.width + x * TILE + tx) * 4) as usize;
                    let to = ((ty * side + tx) * 4) as usize;
                    let differs = image.data()[from..from + 4].iter().zip(&reference.data()[to..to + 4]).any(|(a, b)| a.abs_diff(*b) > 4);
                    changed_full += usize::from(differs);
                    changed_center += usize::from(differs && tx >= bleed && tx < bleed + TILE && ty >= bleed && ty < bleed + TILE);
                }
            }
            println!(
                "tile=({}, {}) independent_ms={:.2} changed_full_pct={:.4} changed_center_pct={:.4}",
                first.0 + x as i32,
                first.1 + y as i32,
                started.elapsed().as_secs_f64() * 1000.0,
                changed_full as f64 * 100.0 / f64::from(side * side),
                changed_center as f64 * 100.0 / f64::from(TILE * TILE)
            );
        }
    }
    Ok(())
}
