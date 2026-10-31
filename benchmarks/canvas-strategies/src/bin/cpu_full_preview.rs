//! Compare one faithful full-viewport raster with repeated tile fallbacks.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use lizard_render::{Options, View, render_view};

fn main() -> Result<(), Box<dyn Error>> {
    let (doc, warnings) = lizard_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let geometry_only = std::env::args().any(|arg| arg == "--geometry");
    let opts = Options { lod: true, skip_effects: geometry_only, ..Default::default() };
    println!("geometry_only={geometry_only}");
    for (label, view) in [
        ("overscan", View { origin: (-448.0, 126.0), scale: 2.0, width: 8192, height: 4096 }),
        ("whole", View { origin: (320.0, 610.0), scale: 2.0, width: 5120, height: 2160 }),
        ("left", View { origin: (320.0, 610.0), scale: 2.0, width: 2560, height: 2160 }),
        ("right", View { origin: (1600.0, 610.0), scale: 2.0, width: 2560, height: 2160 }),
        ("tile", View { origin: (320.0, 610.0), scale: 2.0, width: 512, height: 512 }),
    ] {
        let started = Instant::now();
        let image = render_view(&doc, page, &view, &opts).ok_or("CPU render failed")?;
        println!("view={label} size={}x{} ms={:.2} bytes={}", view.width, view.height, started.elapsed().as_secs_f64() * 1000.0, image.data().len());
    }
    Ok(())
}
