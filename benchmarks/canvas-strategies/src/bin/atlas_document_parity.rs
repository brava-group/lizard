//! Compare a bounded, faithful tile atlas with the document renderer on a saved file.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use canvas_strategies_bench::geometry_atlas::GeometryAtlas;
use canvas_strategies_bench::gpu_scene::GpuCanvas;
use lizard_color::{Color, Paint};
use lizard_doc::{EffectKind, NodeKind};
use lizard_render::{Options, View, render_view};

const SIDE: u32 = 1024;

fn changed_percent(actual: &[u8], reference: &[u8], threshold: u8) -> f64 {
    let changed = actual
        .chunks_exact(4)
        .zip(reference.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > threshold))
        .count();
    changed as f64 * 100.0 / f64::from(SIDE * SIDE)
}

fn wait_for_view(atlas: &mut GeometryAtlas, view: &View) -> Result<f64, Box<dyn Error>> {
    let started = Instant::now();
    atlas.request_margin(view, 0)?;
    while !atlas.covers(view) {
        if started.elapsed() > Duration::from_secs(30) {
            return Err("atlas did not cover the view within 30 seconds".into());
        }
        atlas.install(view, 4)?;
        atlas.request_margin(view, 0)?;
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args().find_map(|arg| arg.strip_prefix("--file=").map(str::to_owned)).ok_or("pass --file=<saved .lizard file>")?;
    let bleed = std::env::args().find_map(|arg| arg.strip_prefix("--bleed=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(128).min(512);
    let slots = std::env::args().find_map(|arg| arg.strip_prefix("--slots=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(32);
    let (doc, warnings) = lizard_format::load_file(Path::new(&path))?;
    if !warnings.is_empty() {
        return Err(format!("document warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("document page missing")?;
    let frame = *doc.top_frames(page).first().ok_or("top frame missing")?;
    let bounds = doc.world_render_bounds(frame).ok_or("frame bounds missing")?;
    let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    let scale = std::env::args()
        .find_map(|arg| arg.strip_prefix("--scale=").and_then(|value| value.parse::<f64>().ok()))
        .unwrap_or_else(|| (f64::from(SIDE) / bounds.width().max(bounds.height()).max(1.0)).clamp(0.25, 4.0));
    if !scale.is_finite() || scale <= 0.0 {
        return Err("scale must be finite and positive".into());
    }
    let view =
        View { origin: (center.0 - f64::from(SIDE) / (2.0 * scale), center.1 - f64::from(SIDE) / (2.0 * scale)), scale, width: SIDE, height: SIDE };
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let reference = render_view(&doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("reference render failed")?;
    let mut atlas_doc = doc.clone();
    if std::env::args().any(|arg| arg == "--no-sparse") {
        let marker = atlas_doc.create(
            page,
            NodeKind::Rectangle,
            "Transparent benchmark bounds",
            view.origin.0 - 1.0,
            view.origin.1 - 1.0,
            f64::from(view.width) / view.scale + 2.0,
            f64::from(view.height) / view.scale + 2.0,
        )?;
        atlas_doc.node_mut(marker).ok_or("benchmark marker missing")?.opacity = 0.0;
    }
    let gpu = GpuCanvas::new(SIDE, SIDE)?;
    let mut atlas = GeometryAtlas::new(Arc::new(atlas_doc), page, gpu.device(), gpu.queue(), SIDE, SIDE, slots, bleed, false)?;
    if std::env::args().any(|arg| arg == "--owner-thread-upload") {
        atlas.upload_on_owner_thread();
    }
    let ready_ms = wait_for_view(&mut atlas, &view)?;
    atlas.render_submit(&view, clear)?;
    let actual = atlas.pixels()?;
    println!("file={path} view_scale={scale:.3} bleed={bleed} ready_ms={ready_ms:.2} stats={:?}", atlas.stats());
    for threshold in [4, 16, 32] {
        println!("threshold={threshold} changed={:.3}%", changed_percent(&actual, reference.data(), threshold));
    }
    if std::env::args().any(|arg| arg == "--save") {
        for (name, pixels) in [("actual", actual.as_slice()), ("reference", reference.data())] {
            let file = std::fs::File::create(format!("target/canvas-strategies/icon-atlas-{name}.png"))?;
            let mut encoder = png::Encoder::new(file, SIDE, SIDE);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(pixels)?;
        }
    }
    if std::env::args().any(|arg| arg == "--effect-edit-check") {
        let target = doc
            .nodes()
            .filter(|node| doc.page_of(node.id) == Some(page))
            .filter(|node| {
                doc.world_render_bounds(node.id).is_some_and(|bounds| {
                    let visible = view.page_rect();
                    bounds.x0 < visible.x1 && bounds.x1 > visible.x0 && bounds.y0 < visible.y1 && bounds.y1 > visible.y0
                })
            })
            .find_map(|node| {
                node.effects
                    .iter()
                    .position(|effect| effect.visible && matches!(effect.kind, EffectKind::DropShadow(_) | EffectKind::LayerBlur(_)))
                    .map(|index| (node.id, index))
            })
            .ok_or("no visible shadow or blur effect in document")?;
        let target_node = doc.node(target.0).ok_or("effect target missing")?;
        println!(
            "effect_target={} node_kind={:?} fills={:?} effect={:?} bounds={:?}",
            target_node.name,
            target_node.kind,
            target_node.fills,
            target_node.effects[target.1].kind,
            doc.world_render_bounds(target.0)
        );
        let cycles = std::env::args()
            .find_map(|arg| arg.strip_prefix("--effect-cycles=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(1)
            .clamp(1, 100);
        let mut current = doc.clone();
        let mut previous_pixels = actual;
        for cycle in 0..cycles {
            let mut edited = current.clone();
            let effect = edited.node_mut(target.0).ok_or("effect node missing")?.effects.get_mut(target.1).ok_or("effect missing")?;
            effect.visible = !effect.visible;
            let edited_reference =
                render_view(&edited, page, &view, &Options { lod: true, ..Default::default() }).ok_or("edited effect reference failed")?;
            atlas.request_margin(&view, 1)?;
            atlas.install(&view, 4)?;
            let started = Instant::now();
            let patched_tiles = if std::env::args().any(|arg| arg == "--auto-invalidate") {
                atlas.invalidate_document_auto(Arc::new(edited.clone()), page)?;
                0
            } else {
                atlas.patch_document_auto(Arc::new(edited.clone()), page)?
            };
            let patch_ms = started.elapsed().as_secs_f64() * 1000.0;
            let edit_ready_ms = wait_for_view(&mut atlas, &view)?;
            atlas.render_submit(&view, clear)?;
            let edited_actual = atlas.pixels()?;
            let difference = changed_percent(&edited_actual, edited_reference.data(), 4);
            let changed_by_edit = changed_percent(&edited_actual, &previous_pixels, 4);
            println!(
                "effect_cycle={cycle} patched_tiles={patched_tiles} patch_ms={patch_ms:.2} edit_ready_ms={edit_ready_ms:.2} edited_reference_difference_4={difference:.3}% changed_by_edit_4={changed_by_edit:.3}% stats={:?}",
                atlas.stats()
            );
            if difference > 5.0 || changed_by_edit < 0.01 {
                return Err("atlas effect edit did not preserve the edited image".into());
            }
            previous_pixels = edited_actual;
            current = edited;
        }
        return Ok(());
    }
    let partial_edit_check = std::env::args().any(|arg| arg == "--partial-edit-check");
    let patch_edit_check = std::env::args().any(|arg| arg == "--patch-edit-check");
    let auto_patch_edit_check = std::env::args().any(|arg| arg == "--auto-patch-edit-check");
    let auto_invalidate_edit_check = std::env::args().any(|arg| arg == "--auto-invalidate-edit-check");
    if partial_edit_check
        || patch_edit_check
        || auto_patch_edit_check
        || auto_invalidate_edit_check
        || std::env::args().any(|arg| arg == "--edit-check")
    {
        let mut edited = doc;
        let marker = edited.create(page, NodeKind::Rectangle, "Atlas edit marker", center.0 - 50.0, center.1 - 50.0, 100.0, 100.0)?;
        edited.node_mut(marker).ok_or("edit marker missing")?.fills = vec![Paint::solid(Color::new(0.95, 0.1, 0.18, 1.0))];
        let damage = edited.world_render_bounds(marker).ok_or("edit marker bounds missing")?;
        let patch_view = View {
            origin: (damage.x0, damage.y0),
            scale: 2.4,
            width: (damage.width() * 2.4).ceil() as u32,
            height: (damage.height() * 2.4).ceil() as u32,
        };
        let patch_started = Instant::now();
        let _patch = render_view(&edited, page, &patch_view, &Options { lod: true, ..Default::default() }).ok_or("edit patch render failed")?;
        let patch_render_ms = patch_started.elapsed().as_secs_f64() * 1000.0;
        let edited_reference = render_view(&edited, page, &view, &Options { lod: true, ..Default::default() }).ok_or("edited reference failed")?;
        atlas.request_margin(&view, 1)?;
        atlas.install(&view, 4)?;
        let patch_started = Instant::now();
        let patched_tiles = if auto_patch_edit_check {
            atlas.patch_document_auto(Arc::new(edited), page)?
        } else if auto_invalidate_edit_check {
            atlas.invalidate_document_auto(Arc::new(edited), page)?;
            0
        } else if patch_edit_check {
            atlas.patch_document(Arc::new(edited), page, &[damage])?
        } else if partial_edit_check {
            atlas.replace_document_with_damage(Arc::new(edited), page, &[damage])?;
            0
        } else {
            atlas.replace_document(Arc::new(edited), page)?;
            0
        };
        let patch_ms = patch_started.elapsed().as_secs_f64() * 1000.0;
        if !patch_edit_check && !auto_patch_edit_check && !auto_invalidate_edit_check && atlas.covers(&view) {
            return Err("stale atlas tiles survived document replacement".into());
        }
        let edit_ready_ms = wait_for_view(&mut atlas, &view)?;
        std::thread::sleep(Duration::from_millis(100));
        atlas.install(&view, 4)?;
        atlas.render_submit(&view, clear)?;
        let edited_actual = atlas.pixels()?;
        let difference = changed_percent(&edited_actual, edited_reference.data(), 4);
        let changed_by_edit = changed_percent(&edited_actual, &actual, 4);
        println!(
            "partial_edit={partial_edit_check} patch_edit={patch_edit_check} auto_patch_edit={auto_patch_edit_check} auto_invalidate={auto_invalidate_edit_check} patched_tiles={patched_tiles} patch_ms={patch_ms:.2} patch_render_ms={patch_render_ms:.2} edit_ready_ms={edit_ready_ms:.2} edited_reference_difference_4={difference:.3}% changed_by_edit_4={changed_by_edit:.3}% stats={:?}",
            atlas.stats()
        );
        if difference > 2.0 || changed_by_edit < 0.1 {
            return Err("atlas edit invalidation did not preserve the edited image".into());
        }
    }
    Ok(())
}
