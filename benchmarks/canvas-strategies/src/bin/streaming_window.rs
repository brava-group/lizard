//! Bounded retained tile cache measured through a real macOS Metal swapchain.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::error::Error;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use canvas_strategies_bench::compositor::Compositor;
use canvas_strategies_bench::gpu_residual::GpuResidual;
use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view};
use canvas_strategies_bench::residual::Residual;
use lizard_doc::{Document, EffectKind, NodeId, NodeKind};
use lizard_render::{Options, Pixmap, View, render_view};
use rayon::prelude::*;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ControlFlow;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowId};

const WIDTH: u32 = 5120;
const HEIGHT: u32 = 2160;
const TILE: u32 = 512;
const SCALE: f64 = 2.0;
const DETAIL: f64 = 2.32;
const DETAIL_SLOTS: usize = 80;
const HZ: f64 = 75.0;
const BLIT: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var sampler_linear: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = array<vec2<f32>, 6>(vec2<f32>(0.0,0.0),vec2<f32>(1.0,0.0),vec2<f32>(0.0,1.0),
                                  vec2<f32>(0.0,1.0),vec2<f32>(1.0,0.0),vec2<f32>(1.0,1.0))[i];
    var out: Out;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}
@fragment fn fs(input: Out) -> @location(0) vec4<f32> {
    return textureSample(source, sampler_linear, input.uv);
}
"#;

#[derive(Clone, Copy, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
struct Key {
    tier: u8,
    x: i32,
    y: i32,
}

type CpuDone = (Key, Result<Vec<u8>, String>, Instant);

impl Key {
    fn scale(self) -> f64 {
        if self.tier == 0 { SCALE } else { DETAIL }
    }

    fn view(self, bleed: u32) -> View {
        let scale = self.scale();
        View {
            origin: ((f64::from(self.x * TILE as i32) - f64::from(bleed)) / scale, (f64::from(self.y * TILE as i32) - f64::from(bleed)) / scale),
            scale,
            width: TILE + bleed * 2,
            height: TILE + bleed * 2,
        }
    }
}

#[derive(Clone, Copy)]
struct Camera {
    center: (f64, f64),
    zoom: f64,
}

impl Camera {
    fn origin(self) -> (f64, f64) {
        (self.center.0 - f64::from(WIDTH) / self.zoom / 2.0, self.center.1 - f64::from(HEIGHT) / self.zoom / 2.0)
    }
}

fn visible(camera: Camera, tier: u8) -> BTreeSet<Key> {
    let origin = camera.origin();
    let scale = if tier == 0 { SCALE } else { DETAIL };
    let grid = |p: f64| (p * scale / f64::from(TILE)).floor() as i32;
    let right = origin.0 + f64::from(WIDTH) / camera.zoom;
    let bottom = origin.1 + f64::from(HEIGHT) / camera.zoom;
    (grid(origin.1)..=grid(bottom)).flat_map(|y| (grid(origin.0)..=grid(right)).map(move |x| Key { tier, x, y })).collect()
}

fn pan_candidates(camera: Camera) -> Vec<Key> {
    let visible = visible(camera, 0);
    let min_x = visible.iter().map(|key| key.x).min().unwrap_or(0) - 4;
    let max_x = visible.iter().map(|key| key.x).max().unwrap_or(0) + 4;
    let min_y = visible.iter().map(|key| key.y).min().unwrap_or(0) - 2;
    let max_y = visible.iter().map(|key| key.y).max().unwrap_or(0) + 2;
    let mut keys: Vec<_> = (min_y..=max_y).flat_map(|y| (min_x..=max_x).map(move |x| Key { tier: 0, x, y })).collect();
    keys.sort_by(|a, b| {
        let rank = |key: &Key| {
            let x = (f64::from(key.x) + 0.5) * f64::from(TILE) / SCALE - camera.center.0;
            let y = (f64::from(key.y) + 0.5) * f64::from(TILE) / SCALE - camera.center.1;
            (!visible.contains(key), x * x + y * y)
        };
        rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    keys
}

fn warm_plan(camera: Camera, base_slots: usize, pan_before_detail: usize) -> Vec<Key> {
    let visible_base = visible(camera, 0);
    let mut keys: Vec<Key> = pan_candidates(camera).into_iter().filter(|key| visible_base.contains(key)).take(base_slots).collect();
    let pan: Vec<Key> =
        pan_candidates(camera).into_iter().filter(|key| !visible_base.contains(key)).take(base_slots.saturating_sub(visible_base.len())).collect();
    let early = pan_before_detail.min(pan.len());
    keys.extend(pan.iter().take(early).copied());
    keys.extend(visible(camera, 1));
    keys.extend(pan.into_iter().skip(early));
    keys
}

fn preview_origin(camera: Camera) -> (f64, f64) {
    let origin = camera.origin();
    let span = f64::from(WIDTH) / SCALE;
    // Two overview tiles cover the initial view plus the pan guard band on both sides.
    (origin.0 - span / 2.0, origin.1 - span / 4.0)
}

fn preview_view(camera: Camera, bleed: u32, column: u32) -> View {
    let scale = f64::from(TILE) * SCALE / f64::from(WIDTH);
    let origin = preview_origin(camera);
    View {
        origin: (origin.0 + f64::from(column * TILE) / scale - f64::from(bleed) / scale, origin.1 - f64::from(bleed) / scale),
        scale,
        width: TILE + bleed * 2,
        height: TILE + bleed * 2,
    }
}

fn single_preview_view(camera: Camera, bleed: u32) -> View {
    let scale = f64::from(TILE) * SCALE / f64::from(WIDTH);
    let origin = camera.origin();
    View {
        origin: (origin.0 - f64::from(bleed) / scale, origin.1 - f64::from(bleed) / scale),
        scale,
        width: TILE + bleed * 2,
        height: TILE + bleed * 2,
    }
}

fn sharp_source_view(cameras: &[Camera], factor: u32, oracle: bool, guard: f64) -> Result<View, Box<dyn Error>> {
    let first = cameras.first().ok_or("empty camera trace")?;
    let mut left = f64::INFINITY;
    let mut top = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    let mut scale = if oracle { first.zoom } else { first.zoom * 1.2 };
    for camera in cameras.iter().take(if oracle { cameras.len() } else { 1 }) {
        let origin = camera.origin();
        left = left.min(origin.0);
        top = top.min(origin.1);
        right = right.max(origin.0 + f64::from(WIDTH) / camera.zoom);
        bottom = bottom.max(origin.1 + f64::from(HEIGHT) / camera.zoom);
        scale = scale.max(camera.zoom);
    }
    scale += 0.02;
    let origin = (left - guard, top - guard);
    let width = (((right + guard - origin.0) * scale).ceil() as u32).next_multiple_of(factor);
    let height = (((bottom + guard - origin.1) * scale).ceil() as u32).next_multiple_of(factor);
    Ok(View { origin, scale, width, height })
}

fn sharp_covers(source: &View, camera: Camera) -> bool {
    let origin = camera.origin();
    let right = source.origin.0 + f64::from(source.width) / source.scale;
    let bottom = source.origin.1 + f64::from(source.height) / source.scale;
    origin.0 >= source.origin.0
        && origin.1 >= source.origin.1
        && origin.0 + f64::from(WIDTH) / camera.zoom <= right
        && origin.1 + f64::from(HEIGHT) / camera.zoom <= bottom
}

struct SharpReady {
    compositor: GpuResidual,
    source: View,
    prepare_ms: f64,
}

fn render_sharp_parallel(doc: &Document, page: NodeId, view: &View, opts: &Options, bands: u32) -> Option<Pixmap> {
    if bands <= 1 {
        return render_view(doc, page, view, opts);
    }
    let rendered: Vec<_> = (0..bands)
        .into_par_iter()
        .map(|index| {
            let start = view.height * index / bands;
            let end = view.height * (index + 1) / bands;
            let sub = View {
                origin: (view.origin.0, view.origin.1 + f64::from(start) / view.scale),
                scale: view.scale,
                width: view.width,
                height: end - start,
            };
            render_view(doc, page, &sub, opts)
        })
        .collect();
    let rendered: Vec<Pixmap> = rendered.into_iter().collect::<Option<_>>()?;
    let mut combined = Pixmap::new(view.width, view.height)?;
    let mut at = 0;
    for band in rendered {
        let end = at + band.data().len();
        combined.data_mut()[at..end].copy_from_slice(band.data());
        at = end;
    }
    Some(combined)
}

fn prepare_sharp_underlay(
    doc: &Document,
    page: NodeId,
    source: View,
    factor: u32,
    bands: u32,
    baked: bool,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_format: Option<wgpu::TextureFormat>,
) -> Result<SharpReady, Box<dyn Error>> {
    let start = Instant::now();
    let low = View { origin: source.origin, scale: source.scale / f64::from(factor), width: source.width / factor, height: source.height / factor };
    let suppressed: Vec<_> = doc
        .nodes()
        .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
        .map(|node| node.id)
        .collect();
    let base_opts = Options { lod: true, skip_effects: true, skip: suppressed, ..Default::default() };
    let stage = Instant::now();
    let low_full = render_view(doc, page, &low, &Options { lod: true, ..Default::default() }).ok_or("sharp underlay low full render failed")?;
    let low_full_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = Instant::now();
    let low_base = render_view(doc, page, &low, &base_opts).ok_or("sharp underlay low geometry render failed")?;
    let low_base_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = Instant::now();
    let mut high_base = render_sharp_parallel(doc, page, &source, &base_opts, bands).ok_or("sharp underlay high geometry render failed")?;
    let high_base_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = Instant::now();
    if baked {
        let residual = Residual::new(low_full.clone(), low_base.clone(), low.clone()).ok_or("sharp underlay low views differ")?;
        residual.apply(&mut high_base, &source);
    }
    let bake_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = Instant::now();
    let compositor = GpuResidual::new(device, queue, source.width, source.height, low.width, low.height, WIDTH, HEIGHT, baked, surface_format);
    let setup_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = Instant::now();
    compositor.upload_images(&high_base, &low_full, &low_base)?;
    let upload_ms = stage.elapsed().as_secs_f64() * 1000.0;
    println!(
        "sharp_prepare bands={bands} baked={baked} low_full_ms={low_full_ms:.1} low_base_ms={low_base_ms:.1} high_base_ms={high_base_ms:.1} bake_ms={bake_ms:.1} setup_ms={setup_ms:.1} upload_ms={upload_ms:.1}"
    );
    Ok(SharpReady { compositor, source, prepare_ms: start.elapsed().as_secs_f64() * 1000.0 })
}

fn render_preview(
    gpu: &mut GpuCanvas,
    doc: &Document,
    page: NodeId,
    backdrop: &BackdropIndex,
    camera: Camera,
    bleed: u32,
    clear: [f32; 3],
    paired: bool,
) -> Result<Vec<wgpu::Texture>, Box<dyn Error>> {
    if !paired {
        let view = single_preview_view(camera, bleed);
        let fallback = has_unhandled_effects_in_view(doc, &view);
        let started = Instant::now();
        if fallback {
            let preview = render_view(doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("CPU effect preview failed")?;
            gpu.upload_premul(preview.data())?;
        } else {
            gpu.render_with_backdrop_blurs_indexed(doc, page, &view, clear, backdrop)?;
        }
        let snapshot = gpu.snapshot_submit();
        println!("preview path={} render_submit_ms={:.2}", if fallback { "cpu_fidelity" } else { "gpu" }, started.elapsed().as_secs_f64() * 1000.0);
        return Ok(vec![snapshot]);
    }
    let view = preview_view(camera, bleed, 0);
    let combined = View { width: TILE * 2 + bleed * 2, ..view };
    let fallback = has_unhandled_effects_in_view(doc, &combined);
    let started = Instant::now();
    let snapshots = if fallback {
        let (left, right) = rayon::join(
            || render_view(doc, page, &preview_view(camera, bleed, 0), &Options { lod: true, ..Default::default() }),
            || render_view(doc, page, &preview_view(camera, bleed, 1), &Options { lod: true, ..Default::default() }),
        );
        let mut tiles = Vec::with_capacity(2);
        for preview in [left, right] {
            let preview = preview.ok_or("CPU effect preview failed")?;
            gpu.upload_premul(preview.data())?;
            tiles.push(gpu.snapshot_submit());
        }
        tiles
    } else {
        let mut tiles = Vec::with_capacity(2);
        for column in 0..2 {
            let view = preview_view(camera, bleed, column);
            gpu.render_with_backdrop_blurs_indexed(doc, page, &view, clear, backdrop)?;
            tiles.push(gpu.snapshot_submit());
        }
        tiles
    };
    println!("preview path={} render_submit_ms={:.2}", if fallback { "cpu_fidelity" } else { "gpu" }, started.elapsed().as_secs_f64() * 1000.0);
    Ok(snapshots)
}

fn render_key(
    gpu: &mut GpuCanvas,
    doc: &Document,
    page: NodeId,
    backdrop: &BackdropIndex,
    key: Key,
    bleed: u32,
    clear: [f32; 3],
) -> Result<wgpu::Texture, Box<dyn Error>> {
    let view = key.view(bleed);
    if has_unhandled_effects_in_view(doc, &view) {
        let tile = render_view(doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("CPU effect fallback failed")?;
        gpu.upload_premul(tile.data())?;
    } else {
        gpu.render_with_backdrop_blurs_indexed(doc, page, &view, clear, backdrop)?;
    }
    Ok(gpu.snapshot_submit())
}

/// CPU effect rasterization must not hold the presentation gate. The main thread can keep
/// presenting cached tiles while the fallback tile is prepared, then briefly lock for upload.
fn render_key_with_gate(
    gpu: &mut GpuCanvas,
    gate: &Mutex<()>,
    doc: &Document,
    page: NodeId,
    backdrop: &BackdropIndex,
    key: Key,
    bleed: u32,
    clear: [f32; 3],
) -> Result<wgpu::Texture, Box<dyn Error>> {
    let view = key.view(bleed);
    if has_unhandled_effects_in_view(doc, &view) {
        let tile = render_view(doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("CPU effect fallback failed")?;
        let _guard = gate.lock().map_err(|_| "GPU presentation gate poisoned")?;
        gpu.upload_premul(tile.data())?;
        Ok(gpu.snapshot_submit())
    } else {
        let _guard = gate.lock().map_err(|_| "GPU presentation gate poisoned")?;
        render_key(gpu, doc, page, backdrop, key, bleed, clear)
    }
}

/// Rasterize a compact group once, then extract independently addressable retained tiles.
/// Every tile keeps its original bleed and pixel origin, so the compositor needs no special case.
fn render_region_tiles(doc: &Document, page: NodeId, keys: &[Key], bleed: u32) -> Result<Vec<(Key, Vec<u8>)>, String> {
    let first = keys.first().ok_or("empty CPU region")?;
    let (min_x, max_x) = keys.iter().fold((first.x, first.x), |(lo, hi), key| (lo.min(key.x), hi.max(key.x)));
    let (min_y, max_y) = keys.iter().fold((first.y, first.y), |(lo, hi), key| (lo.min(key.y), hi.max(key.y)));
    let scale = first.scale();
    let width = (max_x - min_x + 1) as u32 * TILE + bleed * 2;
    let height = (max_y - min_y + 1) as u32 * TILE + bleed * 2;
    let view = View {
        origin: ((f64::from(min_x * TILE as i32) - f64::from(bleed)) / scale, (f64::from(min_y * TILE as i32) - f64::from(bleed)) / scale),
        scale,
        width,
        height,
    };
    let image = render_view(doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("CPU region render failed")?;
    let side = (TILE + bleed * 2) as usize;
    let mut result = Vec::with_capacity(keys.len());
    for &key in keys {
        let x = (key.x - min_x) as usize * TILE as usize;
        let y = (key.y - min_y) as usize * TILE as usize;
        let mut pixels = vec![0u8; side * side * 4];
        for row in 0..side {
            let from = ((y + row) * width as usize + x) * 4;
            let to = row * side * 4;
            pixels[to..to + side * 4].copy_from_slice(&image.data()[from..from + side * 4]);
        }
        result.push((key, pixels));
    }
    Ok(result)
}

fn spawn_region(pool: &rayon::ThreadPool, doc: &Arc<Document>, page: NodeId, keys: Vec<Key>, bleed: u32, done: &Sender<CpuDone>) {
    let job_doc = Arc::clone(doc);
    let job_tx = done.clone();
    pool.spawn(move || {
        let started = Instant::now();
        match render_region_tiles(&job_doc, page, &keys, bleed) {
            Ok(tiles) => {
                for (key, pixels) in tiles {
                    let _ = job_tx.send((key, Ok(pixels), started));
                }
            }
            Err(error) => {
                for key in keys {
                    let _ = job_tx.send((key, Err(error.clone()), started));
                }
            }
        }
    });
}

fn verify_composition(
    doc: &Document,
    page: NodeId,
    compositor: &Compositor,
    camera: Camera,
    clear: (f64, f64, f64),
    label: &str,
) -> Result<(), Box<dyn Error>> {
    compositor.render(camera.origin(), camera.zoom, clear)?;
    let actual = compositor.pixels()?;
    let view = View { origin: camera.origin(), scale: camera.zoom, width: WIDTH, height: HEIGHT };
    let reference = render_view(doc, page, &view, &Options { lod: true, ..Default::default() }).ok_or("quality reference failed")?;
    let changed = actual
        .chunks_exact(4)
        .zip(reference.data().chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > 4))
        .count();
    let share = changed as f64 * 100.0 / f64::from(WIDTH * HEIGHT);
    println!("quality_view={label} zoom={:.3} pixels_over_4={share:.3}%", camera.zoom);
    if std::env::args().any(|arg| arg == "--quality-map") {
        let mut row_buckets = vec![0usize; HEIGHT.div_ceil(TILE) as usize];
        let mut column_buckets = vec![0usize; WIDTH.div_ceil(TILE) as usize];
        for (pixel, (a, b)) in actual.chunks_exact(4).zip(reference.data().chunks_exact(4)).enumerate() {
            if a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > 4) {
                row_buckets[pixel / WIDTH as usize / TILE as usize] += 1;
                column_buckets[pixel % WIDTH as usize / TILE as usize] += 1;
            }
        }
        println!("quality_map={label} rows={row_buckets:?} columns={column_buckets:?}");
    }
    if share > 5.0 {
        return Err("streaming atlas failed the gross quality gate".into());
    }
    Ok(())
}

struct TileReady {
    key: Key,
    texture: wgpu::Texture,
    ms: f64,
}

fn publish_tile(
    key: Key,
    result: Result<wgpu::Texture, String>,
    ms: f64,
    ready_tx: &Sender<Result<Ready, String>>,
    proxy: &EventLoopProxy<()>,
    warm_set: &HashSet<Key>,
    rendered: &mut HashSet<Key>,
    warm_sent: &mut usize,
) -> bool {
    let new_warm_tile = result.is_ok() && warm_set.contains(&key) && !rendered.contains(&key);
    let failed = result.is_err();
    let ready = result.map(|texture| Ready::Tile(TileReady { key, texture, ms }));
    if ready.is_ok() {
        rendered.insert(key);
    }
    if ready_tx.send(ready).is_err() {
        return false;
    }
    if new_warm_tile {
        *warm_sent += 1;
    }
    if failed || !new_warm_tile || warm_sent.is_multiple_of(4) || *warm_sent == warm_set.len() {
        let _ = proxy.send_event(());
    }
    true
}

fn needs_render(key: Key, live: bool, rendered: &HashSet<Key>, pending: &HashSet<Key>) -> bool {
    // `rendered` prevents duplicate warm-up work. A live request originates from the
    // atlas, so a previously rendered tile can have been evicted and must be rebuilt.
    !pending.contains(&key) && (live || !rendered.contains(&key))
}

enum Ready {
    Preview(Vec<wgpu::Texture>),
    Tile(TileReady),
}

#[derive(Clone, Copy)]
struct Slot {
    key: Option<Key>,
    last_touch: usize,
}

struct Cache {
    slots: Vec<Slot>,
    resident: HashMap<Key, usize>,
    base_slots: usize,
    evictions: usize,
    forced_evictions: usize,
    atlas_offset: u32,
}

impl Cache {
    fn new(capacity: usize, detail_slots: usize, atlas_offset: u32) -> Result<Self, Box<dyn Error>> {
        if capacity < detail_slots + 56 {
            return Err(format!("cache needs at least {} slots to cover the initial viewport", detail_slots + 56).into());
        }
        Ok(Self {
            slots: vec![Slot { key: None, last_touch: 0 }; capacity],
            resident: HashMap::new(),
            base_slots: capacity - detail_slots,
            evictions: 0,
            forced_evictions: 0,
            atlas_offset,
        })
    }

    fn contains(&self, key: &Key) -> bool {
        self.resident.contains_key(key)
    }

    fn touch(&mut self, camera: Camera, tick: usize) {
        for key in visible(camera, 0).into_iter().chain(if camera.zoom > SCALE + 0.02 { visible(camera, 1) } else { BTreeSet::new() }) {
            if let Some(&slot) = self.resident.get(&key) {
                self.slots[slot].last_touch = tick;
            }
        }
    }

    fn insert(&mut self, key: Key, texture: &wgpu::Texture, compositor: &Compositor, camera: Camera, tick: usize) -> Result<(), Box<dyn Error>> {
        if self.contains(&key) {
            return Ok(());
        }
        let range = if key.tier == 0 { 0..self.base_slots } else { self.base_slots..self.slots.len() };
        let protected: HashSet<Key> = visible(camera, key.tier).into_iter().collect();
        let empty = range.clone().find(|&index| self.slots[index].key.is_none());
        let unused = range
            .clone()
            .filter(|&index| self.slots[index].key.is_some_and(|old| !protected.contains(&old)))
            .min_by_key(|&index| self.slots[index].last_touch);
        let forced = range.clone().min_by_key(|&index| self.slots[index].last_touch);
        let slot = empty.or(unused).or(forced).ok_or("cache tier has no slots")?;
        if let Some(old) = self.slots[slot].key {
            self.resident.remove(&old);
            self.evictions += 1;
            if protected.contains(&old) {
                self.forced_evictions += 1;
            }
        }
        let scale = key.scale() as f32;
        compositor.update_gpu_layer(
            slot as u32 + self.atlas_offset,
            (key.x as f32 * TILE as f32 / scale, key.y as f32 * TILE as f32 / scale),
            TILE as f32 / scale,
            if key.tier == 0 { 0.0 } else { (SCALE + 0.02) as f32 },
            texture,
        )?;
        self.slots[slot] = Slot { key: Some(key), last_touch: tick };
        self.resident.insert(key, slot);
        Ok(())
    }
}

fn trace() -> Vec<Camera> {
    let mut result = Vec::with_capacity(120);
    for i in 0..60 {
        let t = i as f64 / 59.0 * std::f64::consts::TAU;
        result.push(Camera { center: (1600.0 + 900.0 * t.sin(), 1150.0 + 550.0 * (t * 0.7).sin()), zoom: SCALE });
    }
    for i in 0..60 {
        let t = i as f64 / 59.0 * std::f64::consts::TAU;
        result.push(Camera { center: (1600.0, 1150.0), zoom: SCALE * (1.0 + 0.16 * t.sin().max(0.0)) });
    }
    result
}

fn random_trace() -> Vec<Camera> {
    // A reproducible pointer-like walk with bounded acceleration. Keep the pan and
    // scroll phases separate so the existing per-phase metrics remain comparable.
    let mut seed = 0x91e1_0da5_83f4_22f1_u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / ((1_u64 << 53) as f64)
    };
    let mut result = Vec::with_capacity(120);
    let mut center = (1600.0, 1150.0);
    let mut velocity = (0.0, 0.0);
    for _ in 0..60 {
        velocity.0 = (velocity.0 * 0.72 + (random() - 0.5) * 110.0).clamp(-120.0, 120.0);
        velocity.1 = (velocity.1 * 0.72 + (random() - 0.5) * 75.0).clamp(-80.0, 80.0);
        center.0 = (center.0 + velocity.0).clamp(700.0, 2500.0);
        center.1 = (center.1 + velocity.1).clamp(600.0, 1700.0);
        result.push(Camera { center, zoom: SCALE });
    }
    let mut zoom = SCALE;
    let mut direction = 1.0;
    for i in 0..60 {
        if i % 8 == 0 {
            direction = -direction;
        }
        zoom = (zoom + direction * (0.012 + random() * 0.026)).clamp(SCALE, DETAIL);
        result.push(Camera { center, zoom });
    }
    result
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    compositor: Compositor,
    cache: Cache,
    request: Option<Sender<Key>>,
    ready: Receiver<Result<Ready, String>>,
    sharp_ready: Receiver<Result<SharpReady, String>>,
    sharp: Option<SharpReady>,
    sharp_ready_ms: Option<f64>,
    sharp_underlay_frames: usize,
    sharp_requested: bool,
    direct_underlay: bool,
    sharp_only: bool,
    present_sequence: Arc<AtomicUsize>,
    interaction_active: Arc<AtomicBool>,
    interaction_finished: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    warm_start_signal: Option<Sender<()>>,
    cancel_worker: Arc<AtomicBool>,
    inflight: HashSet<Key>,
    gpu_gate: Arc<Mutex<()>>,
    blit: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    clear: (f64, f64, f64),
    cameras: Vec<Camera>,
    boot_started: Instant,
    started: Instant,
    frame: usize,
    latencies: Vec<f64>,
    late: Vec<f64>,
    zoom_phase: Vec<bool>,
    gpu_done: Arc<Mutex<Vec<Option<f64>>>>,
    base_missing_frames: usize,
    detail_stale_frames: usize,
    base_missing_streak: usize,
    detail_stale_streak: usize,
    max_base_missing_streak: usize,
    max_detail_stale_streak: usize,
    missing_base_tiles: usize,
    missing_detail_tiles: usize,
    completed_jobs: usize,
    discarded_jobs: usize,
    job_times: Vec<f64>,
    warm_ms: f64,
    warm_in_background: bool,
    interact_during_warm: bool,
    interact_at_preview: bool,
    interactive_started: bool,
    warm_remaining: HashSet<Key>,
    base_warm_remaining: HashSet<Key>,
    initial_visible_remaining: HashSet<Key>,
    warm_total: usize,
    warm_started: Option<Instant>,
    first_present_ms: Option<f64>,
    first_visual_ms: Option<f64>,
    initial_visible_ready_ms: Option<f64>,
    interactive_ready_ms: Option<f64>,
    full_ready_ms: Option<f64>,
    warm_frames: usize,
    verify_doc: Option<Arc<Document>>,
    verify_page: NodeId,
    atlas_mib: f64,
    preview_slots: u32,
    stress: bool,
    stress_gap_ms: u64,
    stress_jobs: Arc<AtomicUsize>,
    predict_steps: usize,
    last_camera: Option<(Camera, usize)>,
    skipped_inputs: usize,
    first_present: bool,
    occluded_since: Option<Instant>,
}

impl State {
    fn new(event_loop: &ActiveEventLoop, proxy: EventLoopProxy<()>) -> Result<Self, Box<dyn Error>> {
        let boot_started = Instant::now();
        let window = Arc::new(event_loop.create_window(
            Window::default_attributes().with_title("Lizard GPU canvas presentation benchmark").with_fullscreen(Some(Fullscreen::Borderless(None))),
        )?);
        window.set_visible(true);
        window.focus_window();
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).ok_or("no surface configuration")?;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

        let (doc, warnings) = lizard_format::load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ultrawide-workbook.lizard"))?;
        if !warnings.is_empty() {
            return Err(format!("fixture warnings: {warnings:?}").into());
        }
        let page = doc.current_page.ok_or("no page")?;
        let doc = Arc::new(doc);
        let clear = match doc.node(page).map(|n| &n.kind) {
            Some(NodeKind::Page { background, .. }) => (f64::from(background.r), f64::from(background.g), f64::from(background.b)),
            _ => (1.0, 1.0, 1.0),
        };
        let trace_repeats = std::env::args()
            .find_map(|arg| arg.strip_prefix("--trace-repeats=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(1)
            .clamp(1, 100);
        let trace_kind = std::env::args().find_map(|arg| arg.strip_prefix("--trace=").map(str::to_owned)).unwrap_or_else(|| "sine".to_owned());
        let base_trace = match trace_kind.as_str() {
            "sine" => trace(),
            "random" => random_trace(),
            "static" => vec![Camera { center: (1600.0, 1150.0), zoom: SCALE }; 120],
            _ => return Err(format!("unknown trace: {trace_kind}").into()),
        };
        let cameras: Vec<Camera> = (0..trace_repeats).flat_map(|_| base_trace.iter().copied()).collect();
        let capacity = std::env::args().find_map(|arg| arg.strip_prefix("--slots=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(160);
        let detail_slots = std::env::args()
            .find_map(|arg| arg.strip_prefix("--detail-slots=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(DETAIL_SLOTS);
        let stress = std::env::args().any(|arg| arg == "--stress");
        let warm_in_background = std::env::args().any(|arg| arg == "--background-warm");
        let interact_during_warm = std::env::args().any(|arg| arg == "--interact-during-warm");
        let interact_at_preview = std::env::args().any(|arg| arg == "--interact-at-preview");
        if interact_at_preview && !interact_during_warm {
            return Err("--interact-at-preview requires --interact-during-warm".into());
        }
        if interact_during_warm && (!warm_in_background || std::env::args().any(|arg| arg == "--verify")) {
            return Err("--interact-during-warm requires --background-warm and cannot run with --verify".into());
        }
        let stress_gap_ms =
            std::env::args().find_map(|arg| arg.strip_prefix("--stress-gap=").and_then(|value| value.parse::<u64>().ok())).unwrap_or(0);
        let cpu_workers = std::env::args()
            .find_map(|arg| arg.strip_prefix("--cpu-workers=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(1)
            .clamp(1, 8);
        let cpu_async = std::env::args().any(|arg| arg == "--cpu-scheduler=async") && cpu_workers > 1;
        let cpu_region_size =
            std::env::args().find_map(|arg| arg.strip_prefix("--cpu-regions=").and_then(|value| value.parse::<i32>().ok())).unwrap_or(0).clamp(0, 4);
        let cpu_regions = cpu_region_size >= 2 && cpu_async && warm_in_background;
        let paired_overview = std::env::args().any(|arg| arg == "--paired-overview");
        let sharp_underlay = std::env::args().any(|arg| arg == "--sharp-underlay");
        let direct_underlay = std::env::args().any(|arg| arg == "--direct-underlay");
        let sharp_only = std::env::args().any(|arg| arg == "--sharp-only");
        let sharp_baked = std::env::args().any(|arg| arg == "--sharp-baked");
        let tile_uploads_per_frame = std::env::args()
            .find_map(|arg| arg.strip_prefix("--tile-uploads-per-frame=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(usize::MAX);
        if tile_uploads_per_frame == 0 {
            return Err("--tile-uploads-per-frame must be positive".into());
        }
        let sharp_factor =
            std::env::args().find_map(|arg| arg.strip_prefix("--sharp-factor=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(2);
        let sharp_bands =
            std::env::args().find_map(|arg| arg.strip_prefix("--sharp-bands=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(1);
        if sharp_underlay && (!warm_in_background || !(2..=5).contains(&sharp_factor)) {
            return Err("--sharp-underlay requires --background-warm and --sharp-factor=2..=5".into());
        }
        if (direct_underlay || sharp_baked || sharp_only) && !sharp_underlay {
            return Err("--direct-underlay, --sharp-baked and --sharp-only require --sharp-underlay".into());
        }
        if !matches!(sharp_bands, 1 | 2 | 4) {
            return Err("--sharp-bands must be 1, 2 or 4".into());
        }
        let preview_slots = if paired_overview { 2 } else { 1 };
        let region_inflight = std::env::args()
            .find_map(|arg| arg.strip_prefix("--region-inflight=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(cpu_workers)
            .clamp(1, cpu_workers);
        let cpu_pool = if cpu_workers > 1 { Some(rayon::ThreadPoolBuilder::new().num_threads(cpu_workers).build()?) } else { None };
        let verify = std::env::args().any(|arg| arg == "--verify");
        let predict_steps =
            std::env::args().find_map(|arg| arg.strip_prefix("--predict=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(0);
        let bleed = std::env::args()
            .find_map(|arg| arg.strip_prefix("--bleed=").and_then(|value| value.parse::<u32>().ok()))
            .unwrap_or_else(|| (doc.max_backdrop_radius() * DETAIL * 1.5 + 3.0).ceil().clamp(3.0, 256.0) as u32);
        if !(3..=512).contains(&bleed) {
            return Err("--bleed must be 3..=512".into());
        }
        let side = TILE + 2 * bleed;
        let mut gpu = GpuCanvas::from_device(device.clone(), queue.clone(), adapter.get_info().name, side, side);
        let image_count = gpu.register_images(&doc)?;
        println!("gpu_image_textures={image_count}");
        let mut cache = Cache::new(capacity, detail_slots, preview_slots)?;
        let mut compositor = Compositor::empty_slots(WIDTH, HEIGHT, TILE, bleed, capacity as u32 + preview_slots, device.clone(), queue.clone())?;
        if direct_underlay {
            compositor.enable_surface(config.format);
        }
        let backdrop = BackdropIndex::new(&doc, page);
        let clear32 = [clear.0 as f32, clear.1 as f32, clear.2 as f32];
        let warm_start = Instant::now();
        let pan_before_detail =
            std::env::args().find_map(|arg| arg.strip_prefix("--pan-before-detail=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(0);
        let warm_keys = warm_plan(cameras[0], cache.base_slots, pan_before_detail);
        let detail_warm = visible(cameras[0], 1);
        let base_warm = warm_keys.len().saturating_sub(detail_warm.len());
        if std::env::args().any(|arg| arg == "--diagnose-routes") {
            let cpu_base = warm_keys.iter().filter(|key| key.tier == 0 && has_unhandled_effects_in_view(&doc, &key.view(bleed))).count();
            let cpu_detail = warm_keys.iter().filter(|key| key.tier == 1 && has_unhandled_effects_in_view(&doc, &key.view(bleed))).count();
            println!("planned_routes cpu_base={cpu_base}/{base_warm} cpu_detail={cpu_detail}/{}", detail_warm.len());
        }
        if !warm_in_background {
            for key in &warm_keys {
                let texture = render_key(&mut gpu, &doc, page, &backdrop, *key, bleed, clear32)?;
                cache.insert(*key, &texture, &compositor, cameras[0], 0)?;
            }
        }
        let warm_ms = if warm_in_background { 0.0 } else { warm_start.elapsed().as_secs_f64() * 1000.0 };
        let atlas_mib = (capacity as u32 + preview_slots) as f64 * f64::from(side * side * 4) / 1_048_576.0;
        if verify && !warm_in_background {
            verify_composition(&doc, page, &compositor, cameras[0], clear, "base")?;
            let max_zoom = cameras.iter().skip(60).copied().max_by(|a, b| a.zoom.total_cmp(&b.zoom)).ok_or("zoom trace missing")?;
            verify_composition(&doc, page, &compositor, max_zoom, clear, "detail")?;
        }
        println!(
            "window_size={}x{} display_scale={} present_mode={:?} slots={capacity} bleed={bleed} base_slots={} detail_slots={detail_slots} planned_base={} planned_detail={} atlas_mib={atlas_mib:.1} initial_sync_warm_ms={warm_ms:.1} background_warm={warm_in_background} stress={stress} stress_gap_ms={stress_gap_ms} predict_steps={predict_steps} trace={trace_kind} trace_repeats={trace_repeats} cpu_workers={cpu_workers} cpu_async={cpu_async} cpu_regions={cpu_region_size} region_inflight={region_inflight} pan_before_detail={pan_before_detail} interact_at_preview={interact_at_preview} paired_overview={paired_overview} sharp_underlay={sharp_underlay} direct_underlay={direct_underlay} sharp_baked={sharp_baked} sharp_factor={sharp_factor} sharp_bands={sharp_bands} tile_uploads_per_frame={tile_uploads_per_frame}",
            size.width,
            size.height,
            window.scale_factor(),
            config.present_mode,
            cache.base_slots,
            base_warm,
            detail_warm.len(),
        );
        let (request_tx, request_rx) = mpsc::channel::<Key>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Ready, String>>();
        let (start_tx, start_rx) = mpsc::channel::<()>();
        let gpu_gate = Arc::new(Mutex::new(()));
        let worker_gate = Arc::clone(&gpu_gate);
        let cancel_worker = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel_worker);
        let present_sequence = Arc::new(AtomicUsize::new(0));
        let worker_present_sequence = Arc::clone(&present_sequence);
        let interaction_active = Arc::new(AtomicBool::new(false));
        let worker_interaction_active = Arc::clone(&interaction_active);
        let interaction_finished = Arc::new(AtomicBool::new(false));
        let worker_interaction_finished = Arc::clone(&interaction_finished);
        let defer_tiles_until_idle = std::env::args().any(|arg| arg == "--defer-tiles-until-idle");
        if defer_tiles_until_idle && !sharp_underlay {
            return Err("--defer-tiles-until-idle requires --sharp-underlay".into());
        }
        let stress_jobs = Arc::new(AtomicUsize::new(0));
        let stress_jobs_worker = Arc::clone(&stress_jobs);
        let worker_warm = if warm_in_background { warm_keys.clone() } else { Vec::new() };
        let worker_warm_set: HashSet<Key> = worker_warm.iter().copied().collect();
        let sharp_proxy = proxy.clone();
        let sharp_doc = Arc::clone(&doc);
        let worker_proxy = proxy;
        let (cpu_done_tx, cpu_done_rx) = mpsc::channel::<CpuDone>();
        let first_camera = cameras[0];
        // The quality reference needs the initial camera's full tile set resident.
        // An early interactive trace can evict those tiles before warm-up ends.
        let verify_doc = if verify && warm_in_background { Some(Arc::clone(&doc)) } else { None };
        let worker = thread::spawn(move || {
            if warm_in_background {
                if start_rx.recv().is_err() || worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                if defer_tiles_until_idle {
                    while !worker_interaction_finished.load(Ordering::Relaxed) {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                }
                if !sharp_underlay {
                    let preview = {
                        let Ok(_guard) = worker_gate.lock() else { return };
                        render_preview(&mut gpu, &doc, page, &backdrop, first_camera, bleed, clear32, paired_overview)
                            .map(Ready::Preview)
                            .map_err(|error| error.to_string())
                    };
                    if ready_tx.send(preview).is_err() {
                        return;
                    }
                    let _ = worker_proxy.send_event(());
                }
            }
            let mut warm_iter = worker_warm.clone().into_iter().peekable();
            let mut rendered = HashSet::new();
            let mut cpu_pending = HashSet::new();
            let mut deferred_live = VecDeque::new();
            let mut warm_regions = HashMap::<(u8, i32, i32), Vec<Key>>::new();
            if cpu_regions {
                for &key in &worker_warm {
                    let group = (key.tier, key.x.div_euclid(cpu_region_size), key.y.div_euclid(cpu_region_size));
                    warm_regions.entry(group).or_default().push(key);
                }
            }
            let mut warm_sent = 0usize;
            let mut pending_upload = None;
            let mut upload_frame = usize::MAX;
            let mut uploaded_this_frame = 0usize;
            let stress_key = Key { tier: 0, x: 5, y: 3 };
            loop {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let current_frame = worker_present_sequence.load(Ordering::Relaxed);
                if current_frame != upload_frame {
                    upload_frame = current_frame;
                    uploaded_this_frame = 0;
                }
                if cpu_async && let Some((key, pixels, started)) = pending_upload.take().or_else(|| cpu_done_rx.try_recv().ok()) {
                    if worker_interaction_active.load(Ordering::Relaxed) && uploaded_this_frame >= tile_uploads_per_frame {
                        pending_upload = Some((key, pixels, started));
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    cpu_pending.remove(&key);
                    let result = pixels.and_then(|bytes| {
                        let _guard = worker_gate.lock().map_err(|_| "GPU presentation gate poisoned".to_string())?;
                        gpu.upload_premul(&bytes).map_err(|error| error.to_string())?;
                        Ok(gpu.snapshot_submit())
                    });
                    uploaded_this_frame += 1;
                    if !publish_tile(
                        key,
                        result,
                        started.elapsed().as_secs_f64() * 1000.0,
                        &ready_tx,
                        &worker_proxy,
                        &worker_warm_set,
                        &mut rendered,
                        &mut warm_sent,
                    ) {
                        break;
                    }
                    continue;
                }
                let live = match request_rx.try_recv() {
                    Ok(key) => Some(key),
                    Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => deferred_live.pop_front(),
                };
                let planned = if live.is_none() && (!cpu_async || cpu_pending.len() < cpu_workers * 2) {
                    warm_iter.find(|key| !rendered.contains(key) && !cpu_pending.contains(key))
                } else {
                    None
                };
                let is_live = live.is_some();
                let key = match live.or(planned).map(Ok).unwrap_or_else(|| request_rx.recv_timeout(Duration::from_millis(2))) {
                    Ok(key) => key,
                    Err(mpsc::RecvTimeoutError::Timeout) if stress => {
                        if render_key_with_gate(&mut gpu, &worker_gate, &doc, page, &backdrop, stress_key, bleed, clear32).is_ok() {
                            stress_jobs_worker.fetch_add(1, Ordering::Relaxed);
                        }
                        if stress_gap_ms > 0 {
                            thread::sleep(Duration::from_millis(stress_gap_ms));
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if !needs_render(key, is_live, &rendered, &cpu_pending) {
                    continue;
                }
                if cpu_regions && let Some(pool) = cpu_pool.as_ref() {
                    let group = (key.tier, key.x.div_euclid(cpu_region_size), key.y.div_euclid(cpu_region_size));
                    let group_side = cpu_region_size as usize;
                    let estimated = warm_regions.get(&group).map_or(group_side * group_side, Vec::len);
                    let pending_limit = region_inflight * group_side * group_side;
                    if cpu_pending.len() + estimated > pending_limit {
                        deferred_live.push_back(key);
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    let keys = warm_regions.remove(&group).unwrap_or_else(|| {
                        let x0 = group.1 * cpu_region_size;
                        let y0 = group.2 * cpu_region_size;
                        (0..cpu_region_size).flat_map(|y| (0..cpu_region_size).map(move |x| Key { tier: key.tier, x: x0 + x, y: y0 + y })).collect()
                    });
                    let keys: Vec<Key> =
                        keys.into_iter().filter(|tile| needs_render(*tile, is_live && *tile == key, &rendered, &cpu_pending)).collect();
                    cpu_pending.extend(keys.iter().copied());
                    spawn_region(pool, &doc, page, keys, bleed, &cpu_done_tx);
                    continue;
                }
                if cpu_async
                    && let Some(pool) = cpu_pool.as_ref()
                    && has_unhandled_effects_in_view(&doc, &key.view(bleed))
                {
                    if cpu_pending.len() >= cpu_workers * 2 {
                        deferred_live.push_back(key);
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    cpu_pending.insert(key);
                    let job_doc = Arc::clone(&doc);
                    let job_tx = cpu_done_tx.clone();
                    let view = key.view(bleed);
                    let started = Instant::now();
                    pool.spawn(move || {
                        let pixels = render_view(&job_doc, page, &view, &Options { lod: true, ..Default::default() })
                            .map(|image| image.data().to_vec())
                            .ok_or_else(|| "CPU effect fallback failed".to_string());
                        let _ = job_tx.send((key, pixels, started));
                    });
                    continue;
                }
                if let Some(pool) = cpu_pool.as_ref()
                    && !cpu_async
                    && has_unhandled_effects_in_view(&doc, &key.view(bleed))
                {
                    let mut batch = vec![key];
                    while batch.len() < cpu_workers {
                        let Some(&next) = warm_iter.peek() else { break };
                        if rendered.contains(&next) {
                            warm_iter.next();
                            continue;
                        }
                        if !has_unhandled_effects_in_view(&doc, &next.view(bleed)) {
                            break;
                        }
                        batch.push(next);
                        warm_iter.next();
                    }
                    let outputs: Vec<_> = pool.install(|| {
                        batch
                            .par_iter()
                            .map(|&key| {
                                let started = Instant::now();
                                let view = key.view(bleed);
                                let pixels = render_view(&doc, page, &view, &Options { lod: true, ..Default::default() })
                                    .map(|image| image.data().to_vec())
                                    .ok_or_else(|| "CPU effect fallback failed".to_string());
                                (key, pixels, started.elapsed().as_secs_f64() * 1000.0)
                            })
                            .collect()
                    });
                    let mut published = true;
                    for (key, pixels, cpu_ms) in outputs {
                        let upload_started = Instant::now();
                        let result = pixels.and_then(|bytes| {
                            let _guard = worker_gate.lock().map_err(|_| "GPU presentation gate poisoned".to_string())?;
                            gpu.upload_premul(&bytes).map_err(|error| error.to_string())?;
                            Ok(gpu.snapshot_submit())
                        });
                        if !publish_tile(
                            key,
                            result,
                            cpu_ms + upload_started.elapsed().as_secs_f64() * 1000.0,
                            &ready_tx,
                            &worker_proxy,
                            &worker_warm_set,
                            &mut rendered,
                            &mut warm_sent,
                        ) {
                            published = false;
                            break;
                        }
                    }
                    if !published {
                        break;
                    }
                    continue;
                }
                let started = Instant::now();
                let result =
                    render_key_with_gate(&mut gpu, &worker_gate, &doc, page, &backdrop, key, bleed, clear32).map_err(|error| error.to_string());
                if !publish_tile(
                    key,
                    result,
                    started.elapsed().as_secs_f64() * 1000.0,
                    &ready_tx,
                    &worker_proxy,
                    &worker_warm_set,
                    &mut rendered,
                    &mut warm_sent,
                ) {
                    break;
                }
            }
        });

        let (sharp_tx, sharp_ready) = mpsc::channel();
        if sharp_underlay {
            let oracle_source = std::env::args().any(|arg| arg == "--oracle-source");
            let source_guard =
                std::env::args().find_map(|arg| arg.strip_prefix("--source-guard=").and_then(|value| value.parse::<f64>().ok())).unwrap_or(512.0);
            if !source_guard.is_finite() || !(0.0..=2048.0).contains(&source_guard) {
                return Err("--source-guard must be finite and within 0..=2048 world units".into());
            }
            let source = sharp_source_view(&cameras, sharp_factor, oracle_source, source_guard)?;
            let covered = cameras.iter().filter(|&&camera| sharp_covers(&source, camera)).count();
            println!(
                "sharp_source oracle={oracle_source} guard={source_guard:.0} covered_cameras={covered}/{} source={}x{}",
                cameras.len(),
                source.width,
                source.height
            );
            if source.width.div_ceil(2) + 2 > device.limits().max_texture_dimension_2d || source.height > device.limits().max_texture_dimension_2d {
                return Err("sharp underlay exceeds GPU texture dimensions".into());
            }
            let sharp_device = device.clone();
            let sharp_queue = queue.clone();
            let surface_format = direct_underlay.then_some(config.format);
            thread::spawn(move || {
                let prepared = prepare_sharp_underlay(
                    &sharp_doc,
                    page,
                    source,
                    sharp_factor,
                    sharp_bands,
                    sharp_baked,
                    sharp_device,
                    sharp_queue,
                    surface_format,
                )
                .map_err(|error| error.to_string());
                let _ = sharp_tx.send(prepared);
                let _ = sharp_proxy.send_event(());
            });
        }

        let shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("present blit"), source: wgpu::ShaderSource::Wgsl(BLIT.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present blit bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present blit"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(compositor.output_view()) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: config.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let gpu_done = Arc::new(Mutex::new(vec![None; cameras.len()]));
        let warm_total = warm_keys.len();
        let base_warm_remaining = if warm_in_background { warm_keys.iter().copied().filter(|key| key.tier == 0).collect() } else { HashSet::new() };
        let initial_visible_remaining = if warm_in_background { visible(cameras[0], 0).into_iter().collect() } else { HashSet::new() };
        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            compositor,
            cache,
            request: Some(request_tx),
            ready: ready_rx,
            sharp_ready,
            sharp: None,
            sharp_ready_ms: None,
            sharp_underlay_frames: 0,
            sharp_requested: sharp_underlay,
            direct_underlay,
            sharp_only,
            present_sequence,
            interaction_active,
            interaction_finished,
            worker: Some(worker),
            warm_start_signal: if warm_in_background { Some(start_tx) } else { None },
            cancel_worker,
            inflight: HashSet::new(),
            gpu_gate,
            blit,
            bind,
            clear,
            cameras,
            boot_started,
            started: Instant::now(),
            frame: 0,
            latencies: Vec::new(),
            late: Vec::new(),
            zoom_phase: Vec::new(),
            gpu_done,
            base_missing_frames: 0,
            detail_stale_frames: 0,
            base_missing_streak: 0,
            detail_stale_streak: 0,
            max_base_missing_streak: 0,
            max_detail_stale_streak: 0,
            missing_base_tiles: 0,
            missing_detail_tiles: 0,
            completed_jobs: 0,
            discarded_jobs: 0,
            job_times: Vec::new(),
            warm_ms,
            warm_in_background,
            interact_during_warm,
            interact_at_preview,
            interactive_started: false,
            warm_remaining: if warm_in_background { warm_keys.into_iter().collect() } else { HashSet::new() },
            base_warm_remaining,
            initial_visible_remaining,
            warm_total,
            warm_started: None,
            first_present_ms: None,
            first_visual_ms: None,
            initial_visible_ready_ms: if warm_in_background { None } else { Some(boot_started.elapsed().as_secs_f64() * 1000.0) },
            interactive_ready_ms: if warm_in_background { None } else { Some(boot_started.elapsed().as_secs_f64() * 1000.0) },
            full_ready_ms: if warm_in_background { None } else { Some(boot_started.elapsed().as_secs_f64() * 1000.0) },
            warm_frames: 0,
            verify_doc,
            verify_page: page,
            atlas_mib,
            preview_slots,
            stress,
            stress_gap_ms,
            stress_jobs,
            predict_steps,
            last_camera: None,
            skipped_inputs: 0,
            first_present: false,
            occluded_since: None,
        })
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        let warming = self.warm_in_background && !self.warm_remaining.is_empty();
        let tracing = (!warming || self.interactive_started) && self.frame < self.cameras.len();
        self.interaction_active.store(tracing, Ordering::Relaxed);
        if tracing {
            let latest = (self.started.elapsed().as_secs_f64() * HZ).floor() as usize;
            let latest = latest.min(self.cameras.len().saturating_sub(1));
            if latest > self.frame {
                self.skipped_inputs += latest - self.frame;
                self.frame = latest;
            }
        }
        let due = if tracing { self.started + Duration::from_secs_f64(self.frame as f64 / HZ) } else { Instant::now() };
        if tracing && let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        let camera = if tracing {
            self.cameras[self.frame]
        } else if self.interactive_started {
            self.cameras[self.cameras.len() - 1]
        } else {
            self.cameras[0]
        };
        let start = Instant::now();
        let late_ms = start.saturating_duration_since(due).as_secs_f64() * 1000.0;
        self.cache.touch(camera, self.frame);
        let mut first_visual_pending = false;
        if let Ok(result) = self.sharp_ready.try_recv() {
            let ready = result.map_err(|message| -> Box<dyn Error> { message.into() })?;
            self.sharp_ready_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
            println!(
                "sharp_underlay_ready_ms={:.1} prepare_ms={:.1} source={}x{}",
                self.sharp_ready_ms.unwrap_or(0.0),
                ready.prepare_ms,
                ready.source.width,
                ready.source.height
            );
            self.sharp = Some(ready);
            if let Some(signal) = self.warm_start_signal.take() {
                self.warm_started = Some(Instant::now());
                signal.send(())?;
            }
            for slot in 0..self.preview_slots {
                self.compositor.hide_gpu_layer(slot)?;
            }
            first_visual_pending = true;
        }
        while let Ok(result) = self.ready.try_recv() {
            match result.map_err(|message| -> Box<dyn Error> { message.into() })? {
                Ready::Preview(textures) => {
                    if self.sharp.is_none() {
                        let origin = if self.preview_slots == 2 { preview_origin(self.cameras[0]) } else { self.cameras[0].origin() };
                        let span = f64::from(WIDTH) / SCALE;
                        for (column, texture) in textures.iter().enumerate() {
                            self.compositor.update_gpu_layer(
                                column as u32,
                                ((origin.0 + column as f64 * span) as f32, origin.1 as f32),
                                span as f32,
                                0.0,
                                texture,
                            )?;
                        }
                    }
                    first_visual_pending = true;
                }
                Ready::Tile(tile) => {
                    self.inflight.remove(&tile.key);
                    self.job_times.push(tile.ms);
                    self.completed_jobs += 1;
                    self.warm_remaining.remove(&tile.key);
                    self.base_warm_remaining.remove(&tile.key);
                    self.initial_visible_remaining.remove(&tile.key);
                    // Keep a completed tile even when the camera moved away while it was
                    // rendering. The trace may return to it, and no duplicate will be sent.
                    self.cache.insert(tile.key, &tile.texture, &self.compositor, camera, self.frame)?;
                    first_visual_pending = true;
                }
            }
        }
        let initial_detail_ready = !self.warm_remaining.iter().any(|key| key.tier == 1);
        let initial_view_ready = self.initial_visible_remaining.is_empty() && initial_detail_ready;
        if warming && self.interactive_ready_ms.is_none() && initial_view_ready {
            self.interactive_ready_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
        }
        if warming
            && self.interact_during_warm
            && !self.interactive_started
            && (initial_view_ready || (self.interact_at_preview && self.first_visual_ms.is_some()))
        {
            if initial_view_ready {
                for slot in 0..self.preview_slots {
                    self.compositor.hide_gpu_layer(slot)?;
                }
            }
            self.started = Instant::now();
            self.interactive_started = true;
        }
        if self.interactive_started && initial_view_ready {
            for slot in 0..self.preview_slots {
                self.compositor.hide_gpu_layer(slot)?;
            }
        }
        if warming && self.warm_remaining.is_empty() {
            self.warm_ms = self.warm_started.map_or(0.0, |started| started.elapsed().as_secs_f64() * 1000.0);
            self.full_ready_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
            for slot in 0..self.preview_slots {
                self.compositor.hide_gpu_layer(slot)?;
            }
            if let Some(doc) = self.verify_doc.take() {
                verify_composition(&doc, self.verify_page, &self.compositor, self.cameras[0], self.clear, "base")?;
                let max_zoom = self.cameras.iter().skip(60).copied().max_by(|a, b| a.zoom.total_cmp(&b.zoom)).ok_or("zoom trace missing")?;
                verify_composition(&doc, self.verify_page, &self.compositor, max_zoom, self.clear, "detail")?;
            }
            if !self.interactive_started {
                self.started = Instant::now();
            }
        }
        let base = visible(camera, 0);
        let detail = if camera.zoom > SCALE + 0.02 { visible(camera, 1) } else { BTreeSet::new() };
        let missing_base = base.iter().filter(|key| !self.cache.contains(key)).count();
        let missing_detail = detail.iter().filter(|key| !self.cache.contains(key)).count();
        if tracing {
            self.missing_base_tiles += missing_base;
            self.missing_detail_tiles += missing_detail;
            self.base_missing_frames += usize::from(missing_base > 0);
            self.detail_stale_frames += usize::from(missing_detail > 0);
            self.base_missing_streak = if missing_base > 0 { self.base_missing_streak + 1 } else { 0 };
            self.detail_stale_streak = if missing_detail > 0 { self.detail_stale_streak + 1 } else { 0 };
            self.max_base_missing_streak = self.max_base_missing_streak.max(self.base_missing_streak);
            self.max_detail_stale_streak = self.max_detail_stale_streak.max(self.detail_stale_streak);
        }
        if tracing {
            // Queue all visible gaps up to a small bound. A single outstanding request
            // serializes independent CPU effect tiles and leaves zoom detail stale.
            let required = base.iter().chain(detail.iter()).copied().filter(|key| !self.cache.contains(key) && !self.inflight.contains(key));
            let predicted = self.last_camera.and_then(|(previous, previous_frame)| {
                let elapsed_frames = self.frame.saturating_sub(previous_frame);
                if self.predict_steps == 0 || elapsed_frames == 0 || self.frame >= 60 {
                    return None;
                }
                let factor = self.predict_steps as f64 / elapsed_frames as f64;
                Some(Camera {
                    center: (
                        camera.center.0 + (camera.center.0 - previous.center.0) * factor,
                        camera.center.1 + (camera.center.1 - previous.center.1) * factor,
                    ),
                    zoom: camera.zoom,
                })
            });
            if let Some(request) = &self.request {
                let mut candidates: Vec<Key> = required.collect();
                if candidates.is_empty() {
                    candidates.extend(
                        predicted
                            .into_iter()
                            .flat_map(|future| visible(future, 0))
                            .filter(|key| !self.cache.contains(key) && !base.contains(key) && !self.inflight.contains(key)),
                    );
                }
                // The user can start scrolling immediately after a pan. Prepare the
                // zoom tier for the *current* camera while there is spare bandwidth,
                // rather than retaining only detail from the document's opening view.
                if candidates.is_empty() && self.frame % 120 < 60 {
                    let mut nearby: Vec<Key> =
                        visible(camera, 1).into_iter().filter(|key| !self.cache.contains(key) && !self.inflight.contains(key)).collect();
                    nearby.sort_by(|a, b| {
                        let distance = |key: &Key| {
                            let scale = key.scale();
                            let x = (f64::from(key.x) + 0.5) * f64::from(TILE) / scale - camera.center.0;
                            let y = (f64::from(key.y) + 0.5) * f64::from(TILE) / scale - camera.center.1;
                            x * x + y * y
                        };
                        distance(a).total_cmp(&distance(b))
                    });
                    candidates.extend(nearby);
                }
                for key in candidates.into_iter().take(8usize.saturating_sub(self.inflight.len())) {
                    request.send(key)?;
                    self.inflight.insert(key);
                }
            }
        }
        if tracing {
            self.last_camera = Some((camera, self.frame));
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.focus_window();
                thread::sleep(Duration::from_millis(50));
                let since = self.occluded_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(8) {
                    return Err("surface remained occluded for 8 seconds".into());
                }
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Err("surface acquisition timed out".into()),
            _ => return Err("surface was lost or invalid".into()),
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        if let Some(sharp) = &self.sharp {
            let camera_view = View { origin: camera.origin(), scale: camera.zoom, width: WIDTH, height: HEIGHT };
            let complete_view = missing_base == 0 && missing_detail == 0;
            if self.direct_underlay {
                sharp.compositor.render_submit_to_surface(&sharp.source, &camera_view, &view)?;
                if !self.sharp_only && complete_view {
                    self.compositor.render_submit_over_to_surface(camera.origin(), camera.zoom, &view)?;
                }
            } else {
                sharp.compositor.render_submit_into(&sharp.source, &camera_view, self.compositor.output_view());
                if !self.sharp_only && complete_view {
                    self.compositor.render_submit_over(camera.origin(), camera.zoom)?;
                }
            }
            if tracing && (missing_base > 0 || missing_detail > 0) {
                self.sharp_underlay_frames += 1;
            }
        } else {
            self.compositor.render_submit(camera.origin(), camera.zoom, self.clear)?;
        }
        if !self.direct_underlay || self.sharp.is_none() {
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present frame") });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("present blit"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.blit);
                pass.set_bind_group(0, &self.bind, &[]);
                pass.draw(0..6, 0..1);
            }
            self.queue.submit([encoder.finish()]);
        }
        if tracing {
            let completed = Arc::clone(&self.gpu_done);
            let index = self.frame;
            self.queue.on_submitted_work_done(move || {
                if let Ok(mut samples) = completed.lock() {
                    samples[index] = Some(due.elapsed().as_secs_f64() * 1000.0);
                }
            });
        }
        self.queue.present(frame);
        if tracing {
            self.present_sequence.fetch_add(1, Ordering::Relaxed);
        }
        // Presentation is asynchronous; a per-frame GPU fence would serialize the swapchain
        // and turn ordinary FIFO backpressure into accumulated input latency.
        self.device.poll(wgpu::PollType::Poll)?;
        self.occluded_since = None;
        if !self.first_present {
            self.first_present_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
            if self.warm_in_background {
                if !self.sharp_requested
                    && let Some(signal) = self.warm_start_signal.take()
                {
                    self.warm_started = Some(Instant::now());
                    signal.send(())?;
                }
            } else {
                self.started = Instant::now();
                self.first_visual_ms = self.first_present_ms;
                self.initial_visible_ready_ms = self.first_present_ms;
                self.interactive_ready_ms = self.first_present_ms;
                self.full_ready_ms = self.first_present_ms;
            }
            self.first_present = true;
        }
        if first_visual_pending && self.first_visual_ms.is_none() {
            self.first_visual_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
        }
        if self.warm_in_background && self.initial_visible_ready_ms.is_none() && self.initial_visible_remaining.is_empty() {
            self.initial_visible_ready_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
        }
        if !tracing {
            self.warm_frames += 1;
            return Ok(());
        }
        self.latencies.push(start.elapsed().as_secs_f64() * 1000.0);
        self.late.push(late_ms);
        self.zoom_phase.push(self.frame % 120 >= 60);
        self.frame += 1;
        if self.frame == self.cameras.len() {
            self.interaction_finished.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    fn report(&mut self) {
        self.cancel_worker.store(true, Ordering::Relaxed);
        self.warm_start_signal.take();
        self.request.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!("tile worker terminated unexpectedly");
        }
        if let Err(error) = self.device.poll(wgpu::PollType::wait_indefinitely()) {
            eprintln!("GPU queue completion: {error}");
        }
        println!(
            "startup background_warm={} interact_during_warm={} first_present_submit_ms={:.1} first_visual_submit_ms={:.1} initial_visible_ready_ms={:.1} interactive_ready_ms={:.1} full_ready_ms={:.1} warm_ms={:.1} warm_tiles={}/{} warm_frames={}",
            self.warm_in_background,
            self.interact_during_warm,
            self.first_present_ms.unwrap_or(0.0),
            self.first_visual_ms.unwrap_or(0.0),
            self.initial_visible_ready_ms.unwrap_or(0.0),
            self.interactive_ready_ms.unwrap_or(0.0),
            self.full_ready_ms.unwrap_or(0.0),
            self.warm_ms,
            self.warm_total.saturating_sub(self.warm_remaining.len()),
            self.warm_total,
            self.warm_frames,
        );
        println!("sharp_underlay_ready_ms={:.1} sharp_underlay_gap_frames={}", self.sharp_ready_ms.unwrap_or(0.0), self.sharp_underlay_frames,);
        for (name, zoom) in [("pan", false), ("zoom", true)] {
            let mut times: Vec<f64> = self.latencies.iter().zip(&self.zoom_phase).filter(|(_, phase)| **phase == zoom).map(|(ms, _)| *ms).collect();
            if times.is_empty() {
                continue;
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{name} frames={} median_present_call_ms={:.2} p95_ms={:.2} max_ms={:.2} over_13ms={} over_50ms={}",
                times.len(),
                times[times.len() / 2],
                times[((times.len() - 1) as f64 * 0.95).ceil() as usize],
                times[times.len() - 1],
                times.iter().filter(|&&ms| ms > 1000.0 / HZ).count(),
                times.iter().filter(|&&ms| ms > 50.0).count()
            );
        }
        let late = self.late.iter().copied().fold(0.0f64, f64::max);
        let over_50 = self.late.iter().filter(|&&value| value > 50.0).count();
        println!(
            "max_input_schedule_lateness_ms={late:.2} schedule_late_over_50ms={over_50}/{} completed_frames={} skipped_inputs={}",
            self.late.len(),
            self.frame,
            self.skipped_inputs
        );
        let mut queue_done: Vec<f64> =
            self.gpu_done.lock().ok().map(|samples| samples.iter().filter_map(|sample| *sample).collect()).unwrap_or_default();
        queue_done.sort_by(f64::total_cmp);
        let mut jobs = self.job_times.clone();
        jobs.sort_by(f64::total_cmp);
        let p95 = |values: &[f64]| -> f64 { if values.is_empty() { 0.0 } else { values[((values.len() - 1) as f64 * 0.95).ceil() as usize] } };
        println!(
            "cache slots={} atlas_mib={:.1} warm_ms={:.1} stress={} stress_gap_ms={} stress_jobs={} predict_steps={} resident={} evictions={} forced_evictions={} completed_jobs={} discarded_jobs={} job_p95_ms={:.2} missing_base_frames={} stale_detail_frames={} max_missing_base_streak_ms={:.1} max_stale_detail_streak_ms={:.1} missing_base_tile_frames={} missing_detail_tile_frames={} queue_done_samples={} queue_done_p95_ms={:.2} queue_done_over_50ms={}",
            self.cache.slots.len(),
            self.atlas_mib,
            self.warm_ms,
            self.stress,
            self.stress_gap_ms,
            self.stress_jobs.load(Ordering::Relaxed),
            self.predict_steps,
            self.cache.resident.len(),
            self.cache.evictions,
            self.cache.forced_evictions,
            self.completed_jobs,
            self.discarded_jobs,
            p95(&jobs),
            self.base_missing_frames,
            self.detail_stale_frames,
            self.max_base_missing_streak as f64 * 1000.0 / HZ,
            self.max_detail_stale_streak as f64 * 1000.0 / HZ,
            self.missing_base_tiles,
            self.missing_detail_tiles,
            queue_done.len(),
            p95(&queue_done),
            queue_done.iter().filter(|&&ms| ms > 50.0).count(),
        );
    }
}

struct App {
    state: Option<State>,
    failed: bool,
    next_watchdog: Option<Instant>,
    proxy: EventLoopProxy<()>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop, self.proxy.clone()) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
                self.next_watchdog = Some(Instant::now() + Duration::from_millis(100));
            }
            Err(error) => {
                eprintln!("window benchmark setup: {error}");
                self.failed = true;
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, (): ()) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else { return };
        if state.window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                state.report();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = state.draw() {
                    eprintln!("window benchmark frame: {error}");
                    self.failed = true;
                    event_loop.exit();
                } else if state.warm_in_background
                    && !state.warm_remaining.is_empty()
                    && (!state.interactive_started || state.frame >= state.cameras.len())
                {
                    // The worker wakes the window when a preview or a batch of tiles is ready.
                } else if state.frame >= state.cameras.len() {
                    state.report();
                    event_loop.exit();
                } else {
                    state.window.request_redraw();
                }
            }
            WindowEvent::Resized(size)
                if size.width > 0 && size.height > 0 && (size.width != state.config.width || size.height != state.config.height) =>
            {
                let gate = Arc::clone(&state.gpu_gate);
                if let Ok(_guard) = gate.lock() {
                    if let Err(error) = state.device.poll(wgpu::PollType::wait_indefinitely()) {
                        eprintln!("resize GPU wait: {error}");
                        self.failed = true;
                        event_loop.exit();
                    } else {
                        state.config.width = size.width;
                        state.config.height = size.height;
                        state.surface.configure(&state.device, &state.config);
                    }
                } else {
                    eprintln!("resize GPU gate was poisoned");
                    self.failed = true;
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_ref() else { return };
        if state.frame >= state.cameras.len() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let now = Instant::now();
        let deadline = self.next_watchdog.get_or_insert(now);
        if now >= *deadline {
            state.window.request_redraw();
            *deadline = now + Duration::from_millis(100);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(*deadline));
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = App { state: None, failed: false, next_watchdog: None, proxy: event_loop.create_proxy() };
    event_loop.run_app(&mut app)?;
    if app.failed {
        return Err("window benchmark did not complete".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_underlay_reports_when_pan_leaves_its_prepared_area() -> Result<(), Box<dyn Error>> {
        let first = Camera { center: (1600.0, 1150.0), zoom: SCALE };
        let later = Camera { center: (3200.0, 1150.0), zoom: SCALE };
        let source = sharp_source_view(&[first, later], 5, false, 512.0)?;
        assert!(sharp_covers(&source, first));
        assert!(!sharp_covers(&source, later));
        let oracle = sharp_source_view(&[first, later], 5, true, 96.0)?;
        assert!(sharp_covers(&oracle, later));
        Ok(())
    }

    #[test]
    fn parallel_sharp_bands_match_one_render() -> Result<(), Box<dyn Error>> {
        let (doc, warnings) = lizard_format::load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ultrawide-workbook.lizard"))?;
        assert!(warnings.is_empty());
        let page = doc.current_page.ok_or("fixture page missing")?;
        let view = View { origin: (320.0, 610.0), scale: 2.0, width: 511, height: 257 };
        let options = Options { lod: true, skip_effects: true, ..Default::default() };
        let full = render_view(&doc, page, &view, &options).ok_or("full render failed")?;
        for bands in [2, 4] {
            let parallel = render_sharp_parallel(&doc, page, &view, &options, bands).ok_or("band render failed")?;
            let mismatched = parallel.data().iter().zip(full.data()).filter(|(a, b)| a != b).count();
            let maximum = parallel.data().iter().zip(full.data()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            assert!(
                mismatched * 1_000 <= full.data().len() && maximum <= 2,
                "bands={bands} mismatched_channels={mismatched} max_channel_error={maximum}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_live_request_rebuilds_an_evicted_tile() {
        let key = Key { tier: 1, x: 4, y: 2 };
        let rendered = HashSet::from([key]);
        assert!(!needs_render(key, false, &rendered, &HashSet::new()));
        assert!(needs_render(key, true, &rendered, &HashSet::new()));
        assert!(!needs_render(key, true, &rendered, &HashSet::from([key])));
    }

    #[test]
    fn warm_plan_prioritizes_visible_tiles_and_keeps_zoom_tier_unique() {
        let camera = Camera { center: (1600.0, 1150.0), zoom: SCALE };
        let plan = warm_plan(camera, 160, 0);
        let visible_base = visible(camera, 0);
        assert!(plan.len() > visible_base.len());
        assert!(plan.iter().take(visible_base.len()).all(|key| visible_base.contains(key)));
        assert_eq!(plan.iter().copied().collect::<HashSet<_>>().len(), plan.len());
        assert!(visible(camera, 1).iter().all(|key| plan.contains(key)));
        assert!(plan.iter().skip(visible_base.len()).take(visible(camera, 1).len()).all(|key| key.tier == 1));
    }

    #[test]
    fn overview_covers_the_initial_viewport_from_its_central_pixels() {
        let camera = Camera { center: (1600.0, 1150.0), zoom: SCALE };
        let bleed = 102;
        let single = single_preview_view(camera, bleed);
        assert!((single.origin.0 + f64::from(bleed) / single.scale - camera.origin().0).abs() < 1e-6);
        assert!((single.origin.1 + f64::from(bleed) / single.scale - camera.origin().1).abs() < 1e-6);
        let first = preview_view(camera, bleed, 0);
        let second = preview_view(camera, bleed, 1);
        let origin = preview_origin(camera);
        let span = f64::from(WIDTH) / SCALE;
        assert!((first.origin.0 + f64::from(bleed) / first.scale - origin.0).abs() < 1e-6);
        assert!((second.origin.0 + f64::from(bleed) / second.scale - origin.0 - span).abs() < 1e-6);
        assert!((first.origin.1 + f64::from(bleed) / first.scale - origin.1).abs() < 1e-6);
        assert!((f64::from(TILE) / first.scale - span).abs() < 1e-6);
        for camera in trace().into_iter().chain(random_trace()) {
            let (x, y) = camera.origin();
            assert!(x >= origin.0 && x + f64::from(WIDTH) / camera.zoom <= origin.0 + 2.0 * span);
            assert!(y >= origin.1 && y + f64::from(HEIGHT) / camera.zoom <= origin.1 + span);
        }
    }
}
