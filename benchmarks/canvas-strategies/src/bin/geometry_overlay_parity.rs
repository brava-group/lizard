//! Test whether a sharp geometry pass can safely cover a low-resolution effect preview.

use std::error::Error;
use std::path::Path;
use std::time::Instant;

use figcraft_color::Color;
use figcraft_doc::EffectKind;
use figcraft_render::{Options, View, render_view};

fn main() -> Result<(), Box<dyn Error>> {
    let icon = std::env::args().any(|arg| arg == "--icon");
    let (width, height, default_factor) = if icon { (1024u32, 1024u32, 4usize) } else { (5120u32, 2160u32, 5usize) };
    let factor = std::env::args()
        .find_map(|arg| arg.strip_prefix("--factor=").and_then(|value| value.parse::<usize>().ok()))
        .unwrap_or(default_factor)
        .clamp(2, 8);
    let path = if icon {
        Path::new("/Users/matheuspires/Documents/Lizard-new-icon.figcraft")
    } else {
        Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft")
    };
    let (doc, warnings) = figcraft_format::load_file(path)?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let suppressed: Vec<_> = doc
        .nodes()
        .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
        .map(|node| node.id)
        .collect();
    println!("suppressed_layer_blur_nodes={}", suppressed.len());
    let page = doc.current_page.ok_or("page missing")?;
    let full_view = if icon {
        let frame = *doc.top_frames(page).first().ok_or("icon frame missing")?;
        let bounds = doc.world_render_bounds(frame).ok_or("icon frame bounds missing")?;
        let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let scale = (f64::from(width) / bounds.width().max(bounds.height()).max(1.0)).clamp(0.25, 4.0);
        View { origin: (center.0 - f64::from(width) / (2.0 * scale), center.1 - f64::from(height) / (2.0 * scale)), scale, width, height }
    } else {
        View { origin: (320.0, 610.0), scale: 2.0, width, height }
    };
    let low_view =
        View { origin: full_view.origin, scale: full_view.scale / factor as f64, width: width / factor as u32, height: height / factor as u32 };
    let opts = Options { lod: true, ..Default::default() };
    let started = Instant::now();
    let low = render_view(&doc, page, &low_view, &opts).ok_or("low preview failed")?;
    let low_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let low_geometry = render_view(&doc, page, &low_view, &Options { skip: suppressed.clone(), skip_effects: true, lod: true, ..Default::default() })
        .ok_or("low geometry preview failed")?;
    let low_geometry_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let geometry =
        render_view(&doc, page, &full_view, &Options { background: Some(Color::TRANSPARENT), skip_effects: true, lod: true, ..Default::default() })
            .ok_or("geometry preview failed")?;
    let geometry_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let geometry_opaque = render_view(&doc, page, &full_view, &Options { skip: suppressed, skip_effects: true, lod: true, ..Default::default() })
        .ok_or("opaque geometry preview failed")?;
    let geometry_opaque_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let reference = render_view(&doc, page, &full_view, &opts).ok_or("reference failed")?;
    let reference_ms = started.elapsed().as_secs_f64() * 1000.0;

    let mut low_changed = [0usize; 2];
    let mut hybrid_changed = [0usize; 2];
    let mut residual_changed = [0usize; 2];
    let mut preview = vec![0u8; width as usize * height as usize * 4];
    let mut hybrid = preview.clone();
    let mut residual = preview.clone();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let px = x as f64 / factor as f64;
            let py = y as f64 / factor as f64;
            let x0 = (px.floor() as usize).min(low_view.width as usize - 1);
            let y0 = (py.floor() as usize).min(low_view.height as usize - 1);
            let x1 = (x0 + 1).min(low_view.width as usize - 1);
            let y1 = (y0 + 1).min(low_view.height as usize - 1);
            let fx = px.fract();
            let fy = py.fract();
            let at = (y * width as usize + x) * 4;
            let low_at = |xx: usize, yy: usize, c: usize| low.data()[(yy * low_view.width as usize + xx) * 4 + c] as f64;
            let difference_at =
                |xx: usize, yy: usize, c: usize| low_at(xx, yy, c) - f64::from(low_geometry.data()[(yy * low_view.width as usize + xx) * 4 + c]);
            for c in 0..4 {
                let top = low_at(x0, y0, c) * (1.0 - fx) + low_at(x1, y0, c) * fx;
                let bottom = low_at(x0, y1, c) * (1.0 - fx) + low_at(x1, y1, c) * fx;
                let base = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
                preview[at + c] = base;
                let opacity = geometry.data()[at + 3] as f64 / 255.0;
                hybrid[at + c] = (f64::from(geometry.data()[at + c]) + f64::from(base) * (1.0 - opacity)).round().min(255.0) as u8;
                let delta_top = difference_at(x0, y0, c) * (1.0 - fx) + difference_at(x1, y0, c) * fx;
                let delta_bottom = difference_at(x0, y1, c) * (1.0 - fx) + difference_at(x1, y1, c) * fx;
                residual[at + c] =
                    (f64::from(geometry_opaque.data()[at + c]) + delta_top * (1.0 - fy) + delta_bottom * fy).round().clamp(0.0, 255.0) as u8;
            }
            for (index, threshold) in [4u8, 16u8].into_iter().enumerate() {
                if preview[at..at + 4].iter().zip(&reference.data()[at..at + 4]).any(|(a, b)| a.abs_diff(*b) > threshold) {
                    low_changed[index] += 1;
                }
                if hybrid[at..at + 4].iter().zip(&reference.data()[at..at + 4]).any(|(a, b)| a.abs_diff(*b) > threshold) {
                    hybrid_changed[index] += 1;
                }
                if residual[at..at + 4].iter().zip(&reference.data()[at..at + 4]).any(|(a, b)| a.abs_diff(*b) > threshold) {
                    residual_changed[index] += 1;
                }
            }
        }
    }
    let total = f64::from(width * height);
    println!(
        "low_ms={low_ms:.2} low_geometry_ms={low_geometry_ms:.2} geometry_ms={geometry_ms:.2} geometry_opaque_ms={geometry_opaque_ms:.2} faithful_ms={reference_ms:.2}"
    );
    println!("low_changed_4={:.3}% low_changed_16={:.3}%", low_changed[0] as f64 * 100.0 / total, low_changed[1] as f64 * 100.0 / total);
    println!("hybrid_changed_4={:.3}% hybrid_changed_16={:.3}%", hybrid_changed[0] as f64 * 100.0 / total, hybrid_changed[1] as f64 * 100.0 / total);
    println!(
        "residual_changed_4={:.3}% residual_changed_16={:.3}%",
        residual_changed[0] as f64 * 100.0 / total,
        residual_changed[1] as f64 * 100.0 / total
    );
    if std::env::args().any(|arg| arg == "--no-images") {
        return Ok(());
    }
    let output = if icon { Path::new("target/canvas-strategies/residual-icon") } else { Path::new("target/canvas-strategies/residual-workbook") };
    std::fs::create_dir_all(output)?;
    for (name, pixels) in [
        ("geometry-low.png", preview.as_slice()),
        ("geometry-hybrid.png", hybrid.as_slice()),
        ("geometry-residual.png", residual.as_slice()),
        ("geometry-reference.png", reference.data()),
    ] {
        let file = std::fs::File::create(output.join(name))?;
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(pixels)?;
    }
    Ok(())
}
