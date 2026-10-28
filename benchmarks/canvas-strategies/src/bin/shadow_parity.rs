//! Parameter sweep for the analytic GPU rectangle shadow against the document renderer.

use std::error::Error;

use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view};
use figcraft_color::{Color, Paint};
use figcraft_doc::{Document, Effect, EffectKind, NodeKind};
use figcraft_render::{Options, View, render_view};

fn main() -> Result<(), Box<dyn Error>> {
    let mut doc = Document::new("GPU rectangle shadow sweep");
    let page = doc.current_page.ok_or("document has no page")?;
    let backdrop = doc.create(page, NodeKind::Rectangle, "Backdrop", 0.0, 0.0, 512.0, 512.0)?;
    doc.node_mut(backdrop).ok_or("backdrop missing")?.fills = vec![Paint::solid(Color::new(0.12, 0.52, 0.77, 1.0))];
    let target = doc.create(page, NodeKind::Rectangle, "Target", 72.0, 68.0, 144.0, 120.0)?;
    let node = doc.node_mut(target).ok_or("target missing")?;
    node.corners.radii = [16.0; 4];
    node.fills = vec![Paint::solid(Color::new(0.92, 0.88, 0.3, 0.78))];
    let mut gpu = GpuCanvas::new(512, 512)?;
    let mut cases = 0usize;
    let mut worst = (0.0, String::new());
    let mut supported_worst = (0.0, String::new());
    let mut sum = 0.0;
    for zoom in [1.0, 2.0] {
        for radius in [0.0, 4.0, 14.0, 32.0] {
            for spread in [0.0, 3.0, 10.0] {
                for opacity in [0.10, 0.25, 0.7] {
                    let mut effect = Effect::drop_shadow();
                    if let EffectKind::DropShadow(shadow) = &mut effect.kind {
                        shadow.radius = radius;
                        shadow.spread = spread;
                        shadow.offset = (8.0, 10.0);
                        shadow.color.a = opacity;
                    }
                    doc.node_mut(target).ok_or("target missing")?.effects = vec![effect];
                    let view = View { origin: (0.0, 0.0), scale: zoom, width: 512, height: 512 };
                    let supported = !has_unhandled_effects_in_view(&doc, &view);
                    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU render failed")?;
                    let backdrop_index = BackdropIndex::new(&doc, page);
                    gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [1.0, 1.0, 1.0], &backdrop_index)?;
                    let actual = gpu.pixels()?;
                    let changed = actual
                        .chunks_exact(4)
                        .zip(reference.data().chunks_exact(4))
                        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > 4))
                        .count();
                    let pct = changed as f64 * 100.0 / 512.0_f64.powi(2);
                    let label = format!("zoom={zoom:.1} radius={radius:.1} spread={spread:.1} alpha={opacity:.2}");
                    println!("case {label} supported={supported} changed_pct={pct:.4}");
                    cases += 1;
                    sum += pct;
                    if pct > worst.0 {
                        worst = (pct, label);
                    }
                    if supported && pct > supported_worst.0 {
                        supported_worst = (pct, format!("zoom={zoom:.1} radius={radius:.1} spread={spread:.1} alpha={opacity:.2}"));
                    }
                }
            }
        }
    }
    doc.node_mut(target).ok_or("target missing")?.visible = false;
    let frame = doc.create(page, NodeKind::Frame, "Opaque frame", 72.0, 68.0, 144.0, 120.0)?;
    let node = doc.node_mut(frame).ok_or("frame missing")?;
    node.corners.radii = [16.0; 4];
    node.fills = vec![Paint::solid(Color::new(0.92, 0.88, 0.3, 1.0))];
    node.clip = true;
    let child = doc.create(frame, NodeKind::Rectangle, "Inner content", 20.0, 20.0, 50.0, 50.0)?;
    doc.node_mut(child).ok_or("child missing")?.fills = vec![Paint::solid(Color::new(0.9, 0.2, 0.2, 1.0))];
    let mut effect = Effect::drop_shadow();
    if let EffectKind::DropShadow(shadow) = &mut effect.kind {
        shadow.radius = 14.0;
        shadow.spread = 3.0;
        shadow.offset = (8.0, 10.0);
    }
    doc.node_mut(frame).ok_or("frame missing")?.effects = vec![effect];
    let view = View { origin: (0.0, 0.0), scale: 2.0, width: 512, height: 512 };
    for clipped in [true, false] {
        doc.node_mut(frame).ok_or("frame missing")?.clip = clipped;
        let supported = !has_unhandled_effects_in_view(&doc, &view);
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU frame render failed")?;
        gpu.render_with_backdrop_blurs_indexed(&doc, page, &view, [1.0, 1.0, 1.0], &BackdropIndex::new(&doc, page))?;
        let actual = gpu.pixels()?;
        let changed = actual
            .chunks_exact(4)
            .zip(reference.data().chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > 4))
            .count();
        let frame_pct = changed as f64 * 100.0 / 512.0_f64.powi(2);
        println!("frame_with_child clipped={clipped} supported={supported} changed_pct={frame_pct:.4}");
        if !supported || frame_pct > 5.0 {
            return Err("analytic frame shadow exceeded the supported parity gate".into());
        }
    }
    println!("summary cases={cases} mean_changed_pct={:.4} worst_changed_pct={:.4} worst_case={}", sum / cases as f64, worst.0, worst.1);
    println!("supported_worst_changed_pct={:.4} supported_worst_case={}", supported_worst.0, supported_worst.1);
    if supported_worst.0 > 5.0 {
        return Err("analytic shadow exceeded the supported parity gate".into());
    }
    Ok(())
}
