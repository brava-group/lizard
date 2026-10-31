//! Compare every effect category with the document renderer on one controlled scene.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view};
use lizard_color::{Color, Paint};
use lizard_doc::{Document, Effect, EffectKind, NodeKind};
use lizard_render::{Options, View, render_view};

const SIDE: u32 = 512;

fn changed_percent(left: &[u8], right: &[u8]) -> f64 {
    let changed = left.chunks_exact(4).zip(right.chunks_exact(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 4)).count();
    changed as f64 * 100.0 / f64::from(SIDE * SIDE)
}

fn save_png(name: &str, kind: &str, pixels: &[u8]) -> Result<(), Box<dyn Error>> {
    let path = format!("target/canvas-strategies/effect-{name}-{kind}.png");
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), SIDE, SIDE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut doc = Document::new("GPU effect parity probe");
    let page = doc.current_page.ok_or("new document has no page")?;
    let back = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 256.0, 256.0)?;
    doc.node_mut(back).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.12, 0.52, 0.77, 1.0))];
    let stripe = doc.create(page, NodeKind::Rectangle, "Backdrop stripe", 104.0, 0.0, 52.0, 256.0)?;
    doc.node_mut(stripe).ok_or("stripe missing")?.fills = vec![Paint::solid(Color::new(0.95, 0.31, 0.17, 1.0))];
    let target = doc.create(page, NodeKind::Rectangle, "Effect target", 72.0, 68.0, 144.0, 120.0)?;
    let node = doc.node_mut(target).ok_or("target missing")?;
    node.fills = vec![Paint::solid(Color::new(0.92, 0.88, 0.3, 0.78))];
    node.corners.radii = [16.0; 4];

    let view = View { origin: (0.0, 0.0), scale: 2.0, width: SIDE, height: SIDE };
    let clear = [1.0, 1.0, 1.0];
    let mut gpu = GpuCanvas::new(SIDE, SIDE)?;
    gpu.register_images(&doc)?;
    let baseline = render_view(&doc, page, &view, &Options::default()).ok_or("baseline CPU render failed")?;
    let cases: [(&str, Option<Effect>); 8] = [
        ("none", None),
        ("dropShadow", Some(Effect::drop_shadow())),
        ("innerShadow", Some(Effect::inner_shadow())),
        ("layerBlur", Some(Effect::layer_blur())),
        ("backgroundBlur", Some(Effect::background_blur())),
        ("noise", Some(Effect::noise())),
        ("texture", Some(Effect::texture())),
        ("glass", Some(Effect::glass())),
    ];
    println!("adapter={} scene={}x{} threshold=4/255", gpu.adapter_name(), SIDE, SIDE);
    for (name, mut effect) in cases {
        if let Some(effect) = &mut effect {
            match &mut effect.kind {
                EffectKind::DropShadow(shadow) | EffectKind::InnerShadow(shadow) => {
                    shadow.radius = 14.0;
                    shadow.spread = 3.0;
                    shadow.offset = (8.0, 10.0);
                }
                EffectKind::LayerBlur(blur) | EffectKind::BackgroundBlur(blur) => blur.radius = 12.0,
                _ => {}
            }
        }
        doc.node_mut(target).ok_or("target missing during effect edit")?.effects = effect.into_iter().collect();
        let cpu_started = Instant::now();
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("effect CPU render failed")?;
        let cpu_ms = cpu_started.elapsed().as_secs_f64() * 1000.0;
        let fallback_required = has_unhandled_effects_in_view(&doc, &view);
        let backdrop = BackdropIndex::new(&doc, page);
        let gpu_started = Instant::now();
        let (_, backdrop_count) = gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, clear, &backdrop)?;
        let pixels = gpu.pixels()?;
        let gpu_ms = gpu_started.elapsed().as_secs_f64() * 1000.0;
        if name == "dropShadow" || name == "none" {
            save_png(name, "cpu", reference.data())?;
            save_png(name, "gpu", &pixels)?;
        }
        println!(
            "effect={name} fallback_required={fallback_required} backdrop_passes={backdrop_count} effect_pixels={:.3}% gpu_vs_cpu_pixels={:.3}% cpu_ms={cpu_ms:.2} gpu_and_readback_ms={gpu_ms:.2}",
            changed_percent(reference.data(), baseline.data()),
            changed_percent(&pixels, reference.data())
        );
    }
    Ok(())
}
