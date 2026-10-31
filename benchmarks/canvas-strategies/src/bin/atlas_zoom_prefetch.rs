//! Compare idle zoom-detail preparation with the same camera trace and atlas budget.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use canvas_strategies_bench::geometry_atlas::GeometryAtlas;
use canvas_strategies_bench::gpu_scene::GpuCanvas;
use lizard_render::View;

fn camera(center: (f64, f64), scale: f64, width: u32, height: u32) -> View {
    View { origin: (center.0 - f64::from(width) / (2.0 * scale), center.1 - f64::from(height) / (2.0 * scale)), scale, width, height }
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(quantile * (sorted.len().saturating_sub(1)) as f64).round() as usize]
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let file = args.iter().find_map(|arg| arg.strip_prefix("--file=")).ok_or("pass --file=<saved .lizard file>")?;
    let width = args.iter().find_map(|arg| arg.strip_prefix("--width=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(1266);
    let height = args.iter().find_map(|arg| arg.strip_prefix("--height=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(1720);
    let capacity = args.iter().find_map(|arg| arg.strip_prefix("--slots=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(144);
    let warm_seconds = args.iter().find_map(|arg| arg.strip_prefix("--warm-seconds=").and_then(|value| value.parse::<f64>().ok())).unwrap_or(4.0);
    let prefetch = args.iter().any(|arg| arg == "--prefetch");
    let prefetch_guarded = args.iter().any(|arg| arg == "--prefetch-guarded");
    let prefetch_before_pan = args.iter().any(|arg| arg == "--prefetch-before-pan");
    let high_first = args.iter().any(|arg| arg == "--high-first");
    let projection_order = if high_first { [(3, 3.21), (2, 1.61), (1, 0.81)] } else { [(1, 0.81), (2, 1.61), (3, 3.21)] };
    let pan = args.iter().any(|arg| arg == "--pan");
    let (doc, warnings) = lizard_format::load_file(Path::new(file))?;
    if !warnings.is_empty() {
        return Err(format!("document warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("document page missing")?;
    let frame = *doc.top_frames(page).first().ok_or("top frame missing")?;
    let bounds = doc.world_render_bounds(frame).ok_or("frame bounds missing")?;
    let center = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    let initial = camera(center, 0.5, width, height);
    let gpu = GpuCanvas::new(width, height)?;
    let mut atlas = GeometryAtlas::new(Arc::new(doc), page, gpu.device(), gpu.queue(), width, height, capacity, 128, false)?;
    atlas.upload_on_owner_thread();
    let warm_start = Instant::now();
    while warm_start.elapsed().as_secs_f64() < warm_seconds {
        atlas.request_margin(&initial, 1)?;
        atlas.install(&initial, 4)?;
        if (!pan || prefetch_before_pan) && (prefetch || prefetch_guarded) && atlas.covers_margin(&initial, 1) {
            for (tier, scale) in projection_order {
                let projected = camera(center, scale, width, height);
                if prefetch_guarded {
                    atlas.prefetch_view_guarded(&projected);
                } else {
                    atlas.prefetch_zoom_tiers(&projected, tier);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(13));
    }
    let initial_covered = atlas.covers(&initial);
    let initial_stats = atlas.stats();
    let initial_overview = atlas.overview_stats();
    let mut motion_center = center;
    let mut pan_covered = 0;
    if pan {
        let start = Instant::now();
        for index in 0..60 {
            motion_center = (center.0 + 250.0 * (index as f64 * 0.22).sin(), center.1 + 160.0 * (index as f64 * 0.17).sin());
            let view = camera(motion_center, 0.5, width, height);
            atlas.request_margin(&view, 1)?;
            atlas.install(&view, 4)?;
            pan_covered += usize::from(atlas.covers(&view));
            let deadline = start + Duration::from_secs_f64((index + 1) as f64 / 75.0);
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
        let idle_start = Instant::now();
        let stopped = camera(motion_center, 0.5, width, height);
        while idle_start.elapsed().as_secs_f64() < warm_seconds {
            atlas.request_margin(&stopped, 1)?;
            atlas.install(&stopped, 4)?;
            if !prefetch_before_pan && (prefetch || prefetch_guarded) && atlas.covers_margin(&stopped, 1) {
                for (tier, scale) in projection_order {
                    let projected = camera(motion_center, scale, width, height);
                    if prefetch_guarded {
                        atlas.prefetch_view_guarded(&projected);
                    } else {
                        atlas.prefetch_zoom_tiers(&projected, tier);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(13));
        }
    }
    let mut covered = 0;
    let mut presentable_covered = 0;
    let mut misses = 0;
    let mut longest_miss = 0;
    let mut presentable_misses = 0;
    let mut longest_presentable_miss = 0;
    let mut missing = Vec::new();
    let mut times = Vec::new();
    let mut peak_overview = (0, 0, 0);
    let start = Instant::now();
    for index in 0..120 {
        let fraction = if index < 60 { index as f64 / 59.0 } else { (119 - index) as f64 / 59.0 };
        let scale = 0.5 * 8.0_f64.powf(fraction);
        let view = camera(motion_center, scale, width, height);
        let begin = Instant::now();
        atlas.request_margin(&view, 1)?;
        atlas.install(&view, 4)?;
        if atlas.covers(&view) {
            covered += 1;
            misses = 0;
        } else {
            misses += 1;
            longest_miss = longest_miss.max(misses);
            missing.push((index, (scale * 100.0).round() / 100.0));
        }
        if atlas.covers_presentable(&view) {
            presentable_covered += 1;
            presentable_misses = 0;
            atlas.render_submit(&view, [0.12; 3])?;
        } else {
            presentable_misses += 1;
            longest_presentable_miss = longest_presentable_miss.max(presentable_misses);
        }
        if index == 59 {
            peak_overview = atlas.overview_stats();
        }
        times.push(begin.elapsed().as_secs_f64() * 1000.0);
        let deadline = start + Duration::from_secs_f64((index + 1) as f64 / 75.0);
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
    println!(
        "prefetch={prefetch} prefetch_guarded={prefetch_guarded} prefetch_before_pan={prefetch_before_pan} high_first={high_first} pan={pan} viewport={width}x{height} slots={capacity} warm_s={warm_seconds:.1} initial_covered={initial_covered} initial_stats={initial_stats:?} initial_overview={initial_overview:?} pan_covered={pan_covered}/{} zoom_covered={covered}/120 presentable_zoom={presentable_covered}/120 longest_zoom_miss={longest_miss} longest_presentable_miss={longest_presentable_miss} state_p95_ms={:.2} state_max_ms={:.2} peak_overview={peak_overview:?} final_overview={:?} final_stats={:?}",
        if pan { 60 } else { 0 },
        percentile(&times, 0.95),
        times.iter().copied().fold(0.0_f64, f64::max),
        atlas.overview_stats(),
        atlas.stats(),
    );
    println!("missing_zoom_indices_and_scales={missing:?}");
    Ok(())
}
