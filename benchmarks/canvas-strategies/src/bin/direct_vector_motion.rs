//! Exercise the shared-device Vello GPU scene on the same saved-icon camera trace as the atlas.

use std::error::Error;
use std::fs::File;
use std::path::Path;
use std::time::Instant;

use canvas_strategies_bench::gpu_scene::{
    GpuCanvas, has_unhandled_effects_in_view, has_unhandled_effects_with_cached_noise_in_view, make_scene_with_images,
};
use figcraft_doc::{EffectKind, NodeKind};
use figcraft_geom::Affine;
use figcraft_render::{Options, View, render_view};

fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut encoder = png::Encoder::new(File::create(path)?, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn camera(center: (f64, f64), scale: f64, width: u32, height: u32) -> View {
    View { origin: (center.0 - f64::from(width) / (2.0 * scale), center.1 - f64::from(height) / (2.0 * scale)), scale, width, height }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(p * (sorted.len().saturating_sub(1)) as f64).round() as usize]
}

fn compare_pan_and_drag(
    doc: &figcraft_doc::Document,
    page: figcraft_doc::NodeId,
    frame: figcraft_doc::NodeId,
    clear: [f32; 3],
    center: (f64, f64),
    width: u32,
    height: u32,
    reuse_translations: bool,
) -> Result<(), Box<dyn Error>> {
    let scale = 0.614;
    let fixed_view = camera(center, scale, width, height);
    println!("direct_gpu_eligible={}", !has_unhandled_effects_with_cached_noise_in_view(doc, &fixed_view, true));
    let mut gpu = GpuCanvas::new(width, height)?;
    gpu.register_images(doc)?;
    gpu.prepare_static_effects(doc)?;
    gpu.render_submit_document(doc, page, &fixed_view, clear)?;
    gpu.wait()?;
    let mut pan_ms = Vec::new();
    let mut drag_ms = Vec::new();
    let mut clone_ms = Vec::new();
    let mut effects_ms = Vec::new();
    let mut scene_ms = Vec::new();
    let mut reuse_hits = 0;
    let mut previous = doc.clone();
    for index in 0..60 {
        let phase = index as f64;
        let view = camera((center.0 + 80.0 * (phase * 0.3).sin(), center.1 + 50.0 * (phase * 0.23).sin()), scale, width, height);
        let start = Instant::now();
        gpu.render_submit_document(doc, page, &view, clear)?;
        gpu.wait()?;
        pan_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    for index in 0..60 {
        let phase = index as f64;
        let start = Instant::now();
        let mut moved = doc.clone();
        if let Some(node) = moved.node_mut(frame) {
            node.transform = Affine::translate((80.0 * (phase * 0.3).sin(), 50.0 * (phase * 0.23).sin())) * node.transform;
        }
        clone_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        let effects_start = Instant::now();
        if reuse_translations && gpu.reuse_translated_effects(&previous, &moved) {
            // Translation leaves image and effect pixels unchanged.
            reuse_hits += 1;
        } else {
            gpu.register_images(&moved)?;
            gpu.prepare_static_effects(&moved)?;
        }
        effects_ms.push(effects_start.elapsed().as_secs_f64() * 1000.0);
        let scene_start = Instant::now();
        gpu.render_submit_document(&moved, page, &fixed_view, clear)?;
        scene_ms.push(scene_start.elapsed().as_secs_f64() * 1000.0);
        gpu.wait()?;
        drag_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        previous = moved;
    }
    if reuse_translations {
        let reused_pixels = gpu.pixels()?;
        gpu.prepare_static_effects(&previous)?;
        gpu.render_submit_document(&previous, page, &fixed_view, clear)?;
        let rebuilt_pixels = gpu.pixels()?;
        let differing_channels = reused_pixels.iter().zip(&rebuilt_pixels).filter(|(left, right)| left != right).count();
        let largest_channel_difference = reused_pixels.iter().zip(&rebuilt_pixels).map(|(left, right)| left.abs_diff(*right)).max().unwrap_or(0);
        println!("translation_cache_parity differing_channels={differing_channels} largest_channel_difference={largest_channel_difference}");
    }
    println!(
        "same_file_pan_drag reuse_translations={reuse_translations} reuse_hits={reuse_hits}/60 viewport={width}x{height} scale={scale:.3} pan_p50={:.2} pan_p95={:.2} drag_p50={:.2} drag_p95={:.2} clone_p50={:.2} effects_p50={:.2} effects_p95={:.2} scene_submit_p50={:.2}",
        percentile(&pan_ms, 0.5),
        percentile(&pan_ms, 0.95),
        percentile(&drag_ms, 0.5),
        percentile(&drag_ms, 0.95),
        percentile(&clone_ms, 0.5),
        percentile(&effects_ms, 0.5),
        percentile(&effects_ms, 0.95),
        percentile(&scene_ms, 0.5),
    );
    Ok(())
}

fn compare_rotate(
    doc: &figcraft_doc::Document,
    page: figcraft_doc::NodeId,
    frame: figcraft_doc::NodeId,
    clear: [f32; 3],
    center: (f64, f64),
    width: u32,
    height: u32,
    reuse_transforms: bool,
    snapshot_prefix: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let view = camera(center, 0.614, width, height);
    let mut gpu = GpuCanvas::new(width, height)?;
    gpu.register_images(doc)?;
    gpu.prepare_static_effects(doc)?;
    gpu.render_submit_document(doc, page, &view, clear)?;
    gpu.wait()?;
    let mut previous = doc.clone();
    let mut frame_ms = Vec::new();
    let mut effects_ms = Vec::new();
    let mut reused = 0;
    let pivot = figcraft_geom::Point::new(center.0, center.1);
    for index in 0..60 {
        let angle = (index as f64 * 0.24).sin() * 0.7;
        let start = Instant::now();
        let mut rotated = doc.clone();
        let original = doc.world_transform(frame);
        let change = Affine::translate(pivot.to_vec2()) * Affine::rotate(angle) * Affine::translate(-pivot.to_vec2());
        rotated.set_world_transform(frame, change * original)?;
        let effects_start = Instant::now();
        if reuse_transforms && gpu.reuse_rotated_effects(&previous, &rotated) {
            reused += 1;
        } else {
            gpu.prepare_static_effects(&rotated)?;
        }
        effects_ms.push(effects_start.elapsed().as_secs_f64() * 1000.0);
        gpu.render_submit_document(&rotated, page, &view, clear)?;
        gpu.wait()?;
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        previous = rotated;
    }
    if reuse_transforms {
        let preview = gpu.pixels()?;
        gpu.prepare_static_effects(&previous)?;
        gpu.render_submit_document(&previous, page, &view, clear)?;
        let rebuilt = gpu.pixels()?;
        let differing = preview.iter().zip(&rebuilt).filter(|(a, b)| a != b).count();
        let large_differences = preview.iter().zip(&rebuilt).filter(|(a, b)| a.abs_diff(**b) > 8).count();
        let max_difference = preview.iter().zip(&rebuilt).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let mean = preview.iter().zip(&rebuilt).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / preview.len().max(1) as f64;
        println!(
            "rotation_preview_parity differing_channels={differing} channels_over_8={large_differences} max_channel_difference={max_difference} mean_channel_difference={mean:.4}"
        );
        if let Some(prefix) = snapshot_prefix {
            write_png(&format!("{prefix}-preview.png"), width, height, &preview)?;
            write_png(&format!("{prefix}-rebuilt.png"), width, height, &rebuilt)?;
        }
    }
    println!(
        "same_file_rotate reuse_transforms={reuse_transforms} reuse_hits={reused}/60 viewport={width}x{height} frame_p50={:.2} frame_p95={:.2} effects_p50={:.2}",
        percentile(&frame_ms, 0.5),
        percentile(&frame_ms, 0.95),
        percentile(&effects_ms, 0.5),
    );
    Ok(())
}

fn compare_scale(
    doc: &figcraft_doc::Document,
    page: figcraft_doc::NodeId,
    frame: figcraft_doc::NodeId,
    clear: [f32; 3],
    center: (f64, f64),
    width: u32,
    height: u32,
    reuse_transforms: bool,
    snapshot_prefix: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let view = camera(center, 0.614, width, height);
    let mut gpu = GpuCanvas::new(width, height)?;
    gpu.register_images(doc)?;
    gpu.prepare_static_effects(doc)?;
    gpu.render_submit_document(doc, page, &view, clear)?;
    gpu.wait()?;
    let mut previous = doc.clone();
    let mut frame_ms = Vec::new();
    let mut effects_ms = Vec::new();
    let mut reused = 0;
    for index in 0..60 {
        let k = 1.0 + 0.35 * (index as f64 * 0.24).sin();
        let start = Instant::now();
        let mut scaled = doc.clone();
        figcraft_engine::cmd::arrange::scale_node(&mut scaled, frame, k)?;
        let [a, b, c, d, x, y] = doc.world_transform(frame).as_coeffs();
        scaled.set_world_transform(frame, Affine::new([a, b, c, d, center.0 + (x - center.0) * k, center.1 + (y - center.1) * k]))?;
        let effects_start = Instant::now();
        if reuse_transforms && gpu.reuse_scaled_effects(&previous, &scaled) {
            reused += 1;
        } else {
            gpu.prepare_static_effects(&scaled)?;
        }
        effects_ms.push(effects_start.elapsed().as_secs_f64() * 1000.0);
        gpu.render_submit_document(&scaled, page, &view, clear)?;
        gpu.wait()?;
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        previous = scaled;
    }
    if reuse_transforms {
        let preview = gpu.pixels()?;
        gpu.prepare_static_effects(&previous)?;
        gpu.render_submit_document(&previous, page, &view, clear)?;
        let rebuilt = gpu.pixels()?;
        let differing = preview.iter().zip(&rebuilt).filter(|(a, b)| a != b).count();
        let large_differences = preview.iter().zip(&rebuilt).filter(|(a, b)| a.abs_diff(**b) > 8).count();
        let max_difference = preview.iter().zip(&rebuilt).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let mean = preview.iter().zip(&rebuilt).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / preview.len().max(1) as f64;
        println!(
            "scale_preview_parity differing_channels={differing} channels_over_8={large_differences} max_channel_difference={max_difference} mean_channel_difference={mean:.4}"
        );
        if let Some(prefix) = snapshot_prefix {
            write_png(&format!("{prefix}-preview.png"), width, height, &preview)?;
            write_png(&format!("{prefix}-rebuilt.png"), width, height, &rebuilt)?;
        }
    }
    println!(
        "same_file_scale reuse_transforms={reuse_transforms} reuse_hits={reused}/60 viewport={width}x{height} frame_p50={:.2} frame_p95={:.2} effects_p50={:.2}",
        percentile(&frame_ms, 0.5),
        percentile(&frame_ms, 0.95),
        percentile(&effects_ms, 0.5),
    );
    Ok(())
}

fn compare_resize(
    doc: &figcraft_doc::Document,
    page: figcraft_doc::NodeId,
    frame: figcraft_doc::NodeId,
    clear: [f32; 3],
    center: (f64, f64),
    width: u32,
    height: u32,
    reuse_effects: bool,
    snapshot_prefix: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let view = camera(center, 0.614, width, height);
    let mut gpu = GpuCanvas::new(width, height)?;
    gpu.register_images(doc)?;
    gpu.prepare_static_effects(doc)?;
    gpu.render_submit_document(doc, page, &view, clear)?;
    gpu.wait()?;
    let original = doc.node(frame).ok_or("resize frame missing")?;
    let (w0, h0) = (original.width, original.height);
    let mut previous = doc.clone();
    let mut frame_ms = Vec::new();
    let mut effects_ms = Vec::new();
    let mut reused = 0;
    for index in 0..60 {
        let phase = index as f64 * 0.24;
        let (w, h) = (w0 * (1.0 + 0.25 * phase.sin()), h0 * (1.0 + 0.18 * phase.sin()));
        let start = Instant::now();
        let mut resized = doc.clone();
        figcraft_engine::cmd::props::resize_node(&mut resized, frame, w, h)?;
        let effects_start = Instant::now();
        if reuse_effects && gpu.reuse_resized_effects(&previous, &resized) {
            reused += 1;
        } else {
            gpu.prepare_static_effects(&resized)?;
        }
        effects_ms.push(effects_start.elapsed().as_secs_f64() * 1000.0);
        gpu.render_submit_document(&resized, page, &view, clear)?;
        gpu.wait()?;
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        previous = resized;
    }
    if reuse_effects {
        let preview = gpu.pixels()?;
        gpu.prepare_static_effects(&previous)?;
        gpu.render_submit_document(&previous, page, &view, clear)?;
        let rebuilt = gpu.pixels()?;
        let differing = preview.iter().zip(&rebuilt).filter(|(a, b)| a != b).count();
        let max_difference = preview.iter().zip(&rebuilt).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let mean = preview.iter().zip(&rebuilt).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / preview.len().max(1) as f64;
        println!("resize_preview_parity differing_channels={differing} max_channel_difference={max_difference} mean_channel_difference={mean:.4}");
        if let Some(prefix) = snapshot_prefix {
            write_png(&format!("{prefix}-preview.png"), width, height, &preview)?;
            write_png(&format!("{prefix}-rebuilt.png"), width, height, &rebuilt)?;
        }
    }
    println!(
        "same_file_resize node={} reuse_effects={reuse_effects} reuse_hits={reused}/60 viewport={width}x{height} frame_p50={:.2} frame_p95={:.2} effects_p50={:.2}",
        frame.0,
        percentile(&frame_ms, 0.5),
        percentile(&frame_ms, 0.95),
        percentile(&effects_ms, 0.5),
    );
    Ok(())
}

fn compare_duplicate(
    doc: &figcraft_doc::Document,
    page: figcraft_doc::NodeId,
    source: figcraft_doc::NodeId,
    clear: [f32; 3],
    center: (f64, f64),
    width: u32,
    height: u32,
    reuse_effects: bool,
    snapshot_prefix: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let view = camera(center, 0.614, width, height);
    let mut gpu = GpuCanvas::new(width, height)?;
    gpu.register_images(doc)?;
    gpu.prepare_static_effects(doc)?;
    gpu.render_submit_document(doc, page, &view, clear)?;
    gpu.wait()?;
    let parent = doc.node(source).and_then(|node| node.parent).ok_or("duplicate source has no parent")?;
    let original_world = doc.world_transform(source);
    let mut previous = doc.clone();
    let mut frame_ms = Vec::new();
    let mut document_ms = Vec::new();
    let mut effects_ms = Vec::new();
    let mut reused = 0;
    for index in 0..60 {
        let phase = index as f64 * 0.24;
        let start = Instant::now();
        let mut duplicated = doc.clone();
        let copy = duplicated.duplicate_into(source, parent, doc.index_in_parent(source).map(|index| index + 1))?;
        duplicated.set_world_transform(copy, Affine::translate((100.0 + 80.0 * phase.sin(), 40.0 + 50.0 * phase.cos())) * original_world)?;
        figcraft_engine::normalize(&mut duplicated, doc);
        document_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        let effects_start = Instant::now();
        if reuse_effects && (gpu.reuse_translated_effects(&previous, &duplicated) || gpu.reuse_duplicated_effects(&previous, &duplicated)) {
            reused += 1;
        } else {
            gpu.register_images(&duplicated)?;
            gpu.prepare_static_effects(&duplicated)?;
        }
        effects_ms.push(effects_start.elapsed().as_secs_f64() * 1000.0);
        gpu.render_submit_document(&duplicated, page, &view, clear)?;
        gpu.wait()?;
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        previous = duplicated;
    }
    if reuse_effects {
        let preview = gpu.pixels()?;
        gpu.prepare_static_effects(&previous)?;
        gpu.render_submit_document(&previous, page, &view, clear)?;
        let rebuilt = gpu.pixels()?;
        let max_difference = preview.iter().zip(&rebuilt).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let mean = preview.iter().zip(&rebuilt).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / preview.len().max(1) as f64;
        println!("duplicate_preview_parity max_channel_difference={max_difference} mean_channel_difference={mean:.4}");
        if let Some(prefix) = snapshot_prefix {
            write_png(&format!("{prefix}-preview.png"), width, height, &preview)?;
            write_png(&format!("{prefix}-rebuilt.png"), width, height, &rebuilt)?;
        }
    }
    println!(
        "same_file_duplicate node={} reuse_effects={reuse_effects} reuse_hits={reused}/60 viewport={width}x{height} frame_p50={:.2} frame_p95={:.2} document_p50={:.2} effects_p50={:.2} effects_p95={:.2}",
        source.0,
        percentile(&frame_ms, 0.5),
        percentile(&frame_ms, 0.95),
        percentile(&document_ms, 0.5),
        percentile(&effects_ms, 0.5),
        percentile(&effects_ms, 0.95),
    );
    Ok(())
}

fn compare_create(
    doc: &figcraft_doc::Document,
    page: figcraft_doc::NodeId,
    parent: figcraft_doc::NodeId,
    clear: [f32; 3],
    center: (f64, f64),
    width: u32,
    height: u32,
    reuse_effects: bool,
    kind: &str,
    snapshot_prefix: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let shape_kind = match kind {
        "rectangle" => NodeKind::Rectangle,
        "ellipse" => NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 },
        "frame" => NodeKind::Frame,
        _ => return Err(format!("unsupported creation benchmark kind: {kind}").into()),
    };
    let view = camera(center, 0.614, width, height);
    let mut gpu = GpuCanvas::new(width, height)?;
    gpu.register_images(doc)?;
    gpu.prepare_static_effects(doc)?;
    gpu.render_submit_document(doc, page, &view, clear)?;
    gpu.wait()?;
    let mut previous = doc.clone();
    let mut frame_ms = Vec::new();
    let mut document_ms = Vec::new();
    let mut effects_ms = Vec::new();
    let mut reused = 0;
    for index in 0..60 {
        let phase = index as f64 * 0.24;
        let start = Instant::now();
        let mut drawn = doc.clone();
        drawn.create(parent, shape_kind.clone(), "New Shape", 60.0, 80.0, 120.0 + 60.0 * phase.sin(), 90.0 + 40.0 * phase.cos())?;
        figcraft_engine::normalize(&mut drawn, doc);
        document_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        let effects_start = Instant::now();
        if reuse_effects && gpu.reuse_unaffected_effects(&previous, &drawn) {
            reused += 1;
        } else {
            gpu.register_images(&drawn)?;
            gpu.prepare_static_effects(&drawn)?;
        }
        effects_ms.push(effects_start.elapsed().as_secs_f64() * 1000.0);
        gpu.render_submit_document(&drawn, page, &view, clear)?;
        gpu.wait()?;
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        previous = drawn;
    }
    if reuse_effects {
        let preview = gpu.pixels()?;
        gpu.prepare_static_effects(&previous)?;
        gpu.render_submit_document(&previous, page, &view, clear)?;
        let rebuilt = gpu.pixels()?;
        let max_difference = preview.iter().zip(&rebuilt).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let mean = preview.iter().zip(&rebuilt).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / preview.len().max(1) as f64;
        println!("create_preview_parity max_channel_difference={max_difference} mean_channel_difference={mean:.4}");
        if let Some(prefix) = snapshot_prefix {
            write_png(&format!("{prefix}-preview.png"), width, height, &preview)?;
            write_png(&format!("{prefix}-rebuilt.png"), width, height, &rebuilt)?;
        }
    }
    println!(
        "same_file_create kind={kind} parent={} reuse_effects={reuse_effects} reuse_hits={reused}/60 viewport={width}x{height} frame_p50={:.2} frame_p95={:.2} document_p50={:.2} effects_p50={:.2} effects_p95={:.2}",
        parent.0,
        percentile(&frame_ms, 0.5),
        percentile(&frame_ms, 0.95),
        percentile(&document_ms, 0.5),
        percentile(&effects_ms, 0.5),
        percentile(&effects_ms, 0.95),
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let file = args.iter().find_map(|arg| arg.strip_prefix("--file=")).ok_or("pass --file=<saved .figcraft file>")?;
    let width = args.iter().find_map(|arg| arg.strip_prefix("--width=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(1266);
    let height = args.iter().find_map(|arg| arg.strip_prefix("--height=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(1720);
    let (mut doc, warnings) = figcraft_format::load_file(Path::new(file))?;
    if !warnings.is_empty() {
        return Err(format!("document warnings: {warnings:?}").into());
    }
    let without = args.iter().find_map(|arg| arg.strip_prefix("--without=")).unwrap_or("");
    let cache_blur = args.iter().any(|arg| arg == "--cache-blur");
    if !without.is_empty() {
        let ids: Vec<_> = doc.nodes().map(|node| node.id).collect();
        for id in ids {
            if let Some(node) = doc.node_mut(id) {
                node.effects.retain(|effect| {
                    !matches!(
                        (&effect.kind, without),
                        (EffectKind::LayerBlur(_), "layer-blur")
                            | (EffectKind::DropShadow(_), "drop-shadow")
                            | (EffectKind::InnerShadow(_), "inner-shadow")
                            | (_, "all")
                    )
                });
            }
        }
    }
    let duplicated_inner = if args.iter().any(|arg| arg == "--duplicate-inner") {
        let page = doc.current_page.ok_or("document page missing")?;
        Some(doc.duplicate_into(figcraft_doc::NodeId(26), page, None)?)
    } else {
        None
    };
    let page = doc.current_page.ok_or("document page missing")?;
    let frame = *doc.top_frames(page).first().ok_or("top frame missing")?;
    let resize_frame = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--node-id=").and_then(|value| value.parse::<u64>().ok()))
        .map(figcraft_doc::NodeId)
        .or(duplicated_inner)
        .unwrap_or(frame);
    let bounds = doc.world_render_bounds(frame).ok_or("frame bounds missing")?;
    let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    if args.iter().any(|arg| arg == "--check-direct-capability") {
        let view = camera(center, 0.614, width, height);
        println!("direct_gpu_eligible={}", !has_unhandled_effects_with_cached_noise_in_view(&doc, &view, true));
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--compare-drag") {
        return compare_pan_and_drag(&doc, page, frame, clear, center, width, height, args.iter().any(|arg| arg == "--reuse-translations"));
    }
    if args.iter().any(|arg| arg == "--compare-rotate") {
        return compare_rotate(
            &doc,
            page,
            frame,
            clear,
            center,
            width,
            height,
            args.iter().any(|arg| arg == "--reuse-transforms"),
            args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")),
        );
    }
    if args.iter().any(|arg| arg == "--compare-scale") {
        return compare_scale(
            &doc,
            page,
            frame,
            clear,
            center,
            width,
            height,
            args.iter().any(|arg| arg == "--reuse-transforms"),
            args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")),
        );
    }
    if args.iter().any(|arg| arg == "--compare-resize") {
        return compare_resize(
            &doc,
            page,
            resize_frame,
            clear,
            center,
            width,
            height,
            args.iter().any(|arg| arg == "--reuse-transforms"),
            args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")),
        );
    }
    if args.iter().any(|arg| arg == "--compare-duplicate") {
        return compare_duplicate(
            &doc,
            page,
            resize_frame,
            clear,
            center,
            width,
            height,
            args.iter().any(|arg| arg == "--reuse-transforms"),
            args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")),
        );
    }
    if args.iter().any(|arg| arg == "--compare-create") {
        return compare_create(
            &doc,
            page,
            resize_frame,
            clear,
            center,
            width,
            height,
            args.iter().any(|arg| arg == "--reuse-transforms"),
            args.iter().find_map(|arg| arg.strip_prefix("--kind=")).unwrap_or("rectangle"),
            args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")),
        );
    }
    let mut gpu = GpuCanvas::new(width, height)?;
    let images = gpu.register_images(&doc)?;
    let setup_started = Instant::now();
    let (cached_blurs, cached_drop_shadows, cached_inner_shadows) = if cache_blur { gpu.prepare_static_effects(&doc)? } else { (0, 0, 0) };
    gpu.wait()?;
    let cache_setup_ms = setup_started.elapsed().as_secs_f64() * 1000.0;
    if args.iter().any(|arg| arg == "--snapshot-only") {
        let view = camera(center, 0.614, width, height);
        gpu.render_submit_document(&doc, page, &view, clear)?;
        let gpu_pixels = gpu.pixels()?;
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU reference render failed")?;
        let max_difference = gpu_pixels.iter().zip(reference.data()).map(|(gpu, cpu)| gpu.abs_diff(*cpu)).max().unwrap_or(0);
        let mean_difference =
            gpu_pixels.iter().zip(reference.data()).map(|(gpu, cpu)| f64::from(gpu.abs_diff(*cpu))).sum::<f64>() / gpu_pixels.len().max(1) as f64;
        println!("gpu_cpu_snapshot max_channel_difference={max_difference} mean_channel_difference={mean_difference:.3}");
        if let Some(prefix) = args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")) {
            write_png(&format!("{prefix}-gpu.png"), width, height, &gpu_pixels)?;
            write_png(&format!("{prefix}-cpu.png"), width, height, reference.data())?;
        }
        return Ok(());
    }
    let mut pan = Vec::new();
    let mut zoom = Vec::new();
    let mut scene_ms = Vec::new();
    let mut submit_ms = Vec::new();
    let mut fallback_frames = 0;
    for index in 0..180 {
        let view = if index < 60 {
            let offset = index as f64;
            camera((center.0 + 250.0 * (offset * 0.22).sin(), center.1 + 160.0 * (offset * 0.17).sin()), 0.5, width, height)
        } else {
            let step = index - 60;
            let fraction = if step < 60 { step as f64 / 59.0 } else { (119 - step) as f64 / 59.0 };
            camera(center, 0.5 * 8.0_f64.powf(fraction), width, height)
        };
        fallback_frames += usize::from(has_unhandled_effects_in_view(&doc, &view));
        let started = Instant::now();
        let scene =
            if cache_blur { gpu.make_scene_document(&doc, page, &view) } else { make_scene_with_images(&doc, page, &view, Some(gpu.image_info())) };
        scene_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        submit_ms.push(gpu.render_submit_with_clear(&scene, clear)?);
        gpu.wait()?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if index < 60 {
            pan.push(elapsed);
        } else {
            zoom.push(elapsed);
        }
    }
    println!(
        "viewport={width}x{height} without={without} images={images} cached_blurs={cached_blurs} cached_drop_shadows={cached_drop_shadows} cached_inner_shadows={cached_inner_shadows} cache_setup_ms={cache_setup_ms:.2} fallback_frames={fallback_frames}/180 pan_median_ms={:.2} pan_p95_ms={:.2} pan_max_ms={:.2} zoom_median_ms={:.2} zoom_p95_ms={:.2} zoom_max_ms={:.2} scene_p95_ms={:.2} submit_p95_ms={:.2} zoom_over_13ms={} zoom_over_50ms={}",
        percentile(&pan, 0.5),
        percentile(&pan, 0.95),
        pan.iter().copied().fold(0.0, f64::max),
        percentile(&zoom, 0.5),
        percentile(&zoom, 0.95),
        zoom.iter().copied().fold(0.0, f64::max),
        percentile(&scene_ms, 0.95),
        percentile(&submit_ms, 0.95),
        zoom.iter().filter(|&&ms| ms > 1000.0 / 75.0).count(),
        zoom.iter().filter(|&&ms| ms > 50.0).count(),
    );
    if let Some(prefix) = args.iter().find_map(|arg| arg.strip_prefix("--snapshot-prefix=")) {
        let snapshot_scale =
            args.iter().find_map(|arg| arg.strip_prefix("--snapshot-scale=").and_then(|value| value.parse::<f64>().ok())).unwrap_or(2.0);
        let view = camera(center, snapshot_scale, width, height);
        if cache_blur {
            gpu.render_submit_document(&doc, page, &view, clear)?;
        } else {
            let scene = make_scene_with_images(&doc, page, &view, Some(gpu.image_info()));
            gpu.render_submit_with_clear(&scene, clear)?;
        }
        gpu.wait()?;
        write_png(&format!("{prefix}-gpu.png"), width, height, &gpu.pixels()?)?;
        let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU reference render failed")?;
        write_png(&format!("{prefix}-cpu.png"), width, height, reference.data())?;
        println!("snapshot_prefix={prefix}");
    }
    Ok(())
}
