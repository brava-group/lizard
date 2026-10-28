//! Bounded retained tile cache measured through a real macOS Metal swapchain.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use canvas_strategies_bench::compositor::Compositor;
use canvas_strategies_bench::gpu_scene::{BackdropIndex, GpuCanvas, has_unhandled_effects_in_view};
use figcraft_doc::{Document, NodeId, NodeKind};
use figcraft_render::{Options, View, render_view};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ControlFlow;
use winit::event_loop::{ActiveEventLoop, EventLoop};
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

fn warm_plan(camera: Camera, base_slots: usize) -> Vec<Key> {
    let mut keys: Vec<Key> = pan_candidates(camera).into_iter().take(base_slots).collect();
    keys.extend(visible(camera, 1));
    keys
}

fn preview_view(camera: Camera, bleed: u32) -> View {
    let scale = f64::from(TILE) * SCALE / f64::from(WIDTH);
    let origin = camera.origin();
    View {
        origin: (origin.0 - f64::from(bleed) / scale, origin.1 - f64::from(bleed) / scale),
        scale,
        width: TILE + bleed * 2,
        height: TILE + bleed * 2,
    }
}

fn render_preview(
    gpu: &mut GpuCanvas,
    doc: &Document,
    page: NodeId,
    backdrop: &BackdropIndex,
    camera: Camera,
    bleed: u32,
    clear: [f32; 3],
) -> Result<wgpu::Texture, Box<dyn Error>> {
    gpu.render_with_backdrop_blurs_indexed(doc, page, &preview_view(camera, bleed), clear, backdrop)?;
    Ok(gpu.snapshot_submit())
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

enum Ready {
    Preview(wgpu::Texture),
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
    fn new(capacity: usize, atlas_offset: u32) -> Result<Self, Box<dyn Error>> {
        if capacity < DETAIL_SLOTS + 56 {
            return Err("cache needs at least 136 slots to cover the initial viewport".into());
        }
        Ok(Self {
            slots: vec![Slot { key: None, last_touch: 0 }; capacity],
            resident: HashMap::new(),
            base_slots: capacity - DETAIL_SLOTS,
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
    worker: Option<thread::JoinHandle<()>>,
    warm_start_signal: Option<Sender<()>>,
    cancel_worker: Arc<AtomicBool>,
    inflight: Option<Key>,
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
    missing_base_tiles: usize,
    missing_detail_tiles: usize,
    completed_jobs: usize,
    discarded_jobs: usize,
    job_times: Vec<f64>,
    warm_ms: f64,
    warm_in_background: bool,
    interact_during_warm: bool,
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
    fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
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

        let (doc, warnings) = figcraft_format::load_file(Path::new("examples/ultrawide-workbook.figcraft"))?;
        if !warnings.is_empty() {
            return Err(format!("fixture warnings: {warnings:?}").into());
        }
        let page = doc.current_page.ok_or("no page")?;
        let doc = Arc::new(doc);
        let clear = match doc.node(page).map(|n| &n.kind) {
            Some(NodeKind::Page { background, .. }) => (f64::from(background.r), f64::from(background.g), f64::from(background.b)),
            _ => (1.0, 1.0, 1.0),
        };
        let cameras = trace();
        let capacity = std::env::args().find_map(|arg| arg.strip_prefix("--slots=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(160);
        let stress = std::env::args().any(|arg| arg == "--stress");
        let warm_in_background = std::env::args().any(|arg| arg == "--background-warm");
        let interact_during_warm = std::env::args().any(|arg| arg == "--interact-during-warm");
        if interact_during_warm && (!warm_in_background || std::env::args().any(|arg| arg == "--verify")) {
            return Err("--interact-during-warm requires --background-warm and cannot run with --verify".into());
        }
        let stress_gap_ms =
            std::env::args().find_map(|arg| arg.strip_prefix("--stress-gap=").and_then(|value| value.parse::<u64>().ok())).unwrap_or(0);
        let verify = std::env::args().any(|arg| arg == "--verify");
        let predict_steps =
            std::env::args().find_map(|arg| arg.strip_prefix("--predict=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(0);
        let bleed = (doc.max_backdrop_radius() * DETAIL * 1.5 + 3.0).ceil().clamp(3.0, 256.0) as u32;
        let side = TILE + 2 * bleed;
        let mut gpu = GpuCanvas::from_device(device.clone(), queue.clone(), adapter.get_info().name, side, side);
        let image_count = gpu.register_images(&doc)?;
        println!("gpu_image_textures={image_count}");
        let mut cache = Cache::new(capacity, 1)?;
        // Slot zero is a temporary low-resolution overview; full-resolution tiles cover it.
        let compositor = Compositor::empty_slots(WIDTH, HEIGHT, TILE, bleed, capacity as u32 + 1, device.clone(), queue.clone())?;
        let backdrop = BackdropIndex::new(&doc, page);
        let clear32 = [clear.0 as f32, clear.1 as f32, clear.2 as f32];
        let warm_start = Instant::now();
        let warm_keys = warm_plan(cameras[0], cache.base_slots);
        let detail_warm = visible(cameras[0], 1);
        let base_warm = warm_keys.len().saturating_sub(detail_warm.len());
        if !warm_in_background {
            for key in &warm_keys {
                let texture = render_key(&mut gpu, &doc, page, &backdrop, *key, bleed, clear32)?;
                cache.insert(*key, &texture, &compositor, cameras[0], 0)?;
            }
        }
        let warm_ms = if warm_in_background { 0.0 } else { warm_start.elapsed().as_secs_f64() * 1000.0 };
        let atlas_mib = (capacity + 1) as f64 * f64::from(side * side * 4) / 1_048_576.0;
        if verify && !warm_in_background {
            verify_composition(&doc, page, &compositor, cameras[0], clear, "base")?;
            let max_zoom = cameras.iter().skip(60).copied().max_by(|a, b| a.zoom.total_cmp(&b.zoom)).ok_or("zoom trace missing")?;
            verify_composition(&doc, page, &compositor, max_zoom, clear, "detail")?;
        }
        println!(
            "window_size={}x{} display_scale={} present_mode={:?} slots={capacity} base_slots={} detail_slots={DETAIL_SLOTS} planned_base={} planned_detail={} atlas_mib={atlas_mib:.1} initial_sync_warm_ms={warm_ms:.1} background_warm={warm_in_background} stress={stress} stress_gap_ms={stress_gap_ms} predict_steps={predict_steps}",
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
        let stress_jobs = Arc::new(AtomicUsize::new(0));
        let stress_jobs_worker = Arc::clone(&stress_jobs);
        let worker_warm = if warm_in_background { warm_keys.clone() } else { Vec::new() };
        let worker_warm_set: HashSet<Key> = worker_warm.iter().copied().collect();
        let worker_window = Arc::clone(&window);
        let first_camera = cameras[0];
        // The quality reference needs the initial camera's full tile set resident.
        // An early interactive trace can evict those tiles before warm-up ends.
        let verify_doc = if verify && warm_in_background { Some(Arc::clone(&doc)) } else { None };
        let worker = thread::spawn(move || {
            if warm_in_background {
                if start_rx.recv().is_err() || worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                let preview = {
                    let Ok(_guard) = worker_gate.lock() else { return };
                    render_preview(&mut gpu, &doc, page, &backdrop, first_camera, bleed, clear32)
                        .map(Ready::Preview)
                        .map_err(|error| error.to_string())
                };
                if ready_tx.send(preview).is_err() {
                    return;
                }
                worker_window.request_redraw();
            }
            let mut warm_iter = worker_warm.into_iter();
            let mut rendered = HashSet::new();
            let mut warm_sent = 0usize;
            let stress_key = Key { tier: 0, x: 5, y: 3 };
            loop {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let live = match request_rx.try_recv() {
                    Ok(key) => Some(key),
                    Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => None,
                };
                let planned = if live.is_none() { warm_iter.find(|key| !rendered.contains(key)) } else { None };
                let key = match live.or(planned).map(Ok).unwrap_or_else(|| request_rx.recv_timeout(Duration::from_millis(2))) {
                    Ok(key) => key,
                    Err(mpsc::RecvTimeoutError::Timeout) if stress => {
                        {
                            let Ok(_guard) = worker_gate.lock() else { break };
                            if render_key(&mut gpu, &doc, page, &backdrop, stress_key, bleed, clear32).is_ok() {
                                stress_jobs_worker.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        if stress_gap_ms > 0 {
                            thread::sleep(Duration::from_millis(stress_gap_ms));
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let started = Instant::now();
                let Ok(_guard) = worker_gate.lock() else { break };
                let result = render_key(&mut gpu, &doc, page, &backdrop, key, bleed, clear32)
                    .map(|texture| Ready::Tile(TileReady { key, texture, ms: started.elapsed().as_secs_f64() * 1000.0 }))
                    .map_err(|error| error.to_string());
                let new_warm_tile = result.is_ok() && worker_warm_set.contains(&key) && !rendered.contains(&key);
                if result.is_ok() {
                    rendered.insert(key);
                }
                let failed = result.is_err();
                if ready_tx.send(result).is_err() {
                    break;
                }
                if new_warm_tile {
                    warm_sent += 1;
                }
                if failed || !new_warm_tile || warm_sent.is_multiple_of(4) || warm_sent == worker_warm_set.len() {
                    worker_window.request_redraw();
                }
            }
        });

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
            worker: Some(worker),
            warm_start_signal: if warm_in_background { Some(start_tx) } else { None },
            cancel_worker,
            inflight: None,
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
            missing_base_tiles: 0,
            missing_detail_tiles: 0,
            completed_jobs: 0,
            discarded_jobs: 0,
            job_times: Vec::new(),
            warm_ms,
            warm_in_background,
            interact_during_warm,
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
        while let Ok(result) = self.ready.try_recv() {
            match result.map_err(|message| -> Box<dyn Error> { message.into() })? {
                Ready::Preview(texture) => {
                    let origin = self.cameras[0].origin();
                    self.compositor.update_gpu_layer(0, (origin.0 as f32, origin.1 as f32), f64::from(WIDTH) as f32 / SCALE as f32, 0.0, &texture)?;
                    first_visual_pending = true;
                }
                Ready::Tile(tile) => {
                    if self.inflight == Some(tile.key) {
                        self.inflight = None;
                    }
                    self.job_times.push(tile.ms);
                    self.completed_jobs += 1;
                    self.warm_remaining.remove(&tile.key);
                    self.base_warm_remaining.remove(&tile.key);
                    self.initial_visible_remaining.remove(&tile.key);
                    if tile.key.tier == 0 && !pan_candidates(camera).contains(&tile.key) {
                        self.discarded_jobs += 1;
                    } else {
                        self.cache.insert(tile.key, &tile.texture, &self.compositor, camera, self.frame)?;
                        first_visual_pending = true;
                    }
                }
            }
        }
        if warming && self.interactive_ready_ms.is_none() && self.base_warm_remaining.is_empty() {
            self.interactive_ready_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
        }
        if warming && self.interact_during_warm && !self.interactive_started && self.base_warm_remaining.is_empty() {
            self.compositor.hide_gpu_layer(0)?;
            self.started = Instant::now();
            self.interactive_started = true;
        }
        if warming && self.warm_remaining.is_empty() {
            self.warm_ms = self.warm_started.map_or(0.0, |started| started.elapsed().as_secs_f64() * 1000.0);
            self.full_ready_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
            self.compositor.hide_gpu_layer(0)?;
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
        }
        if tracing && self.inflight.is_none() {
            let required = base.iter().chain(detail.iter()).copied().find(|key| !self.cache.contains(key));
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
            let next = required
                .or_else(|| predicted.and_then(|future| visible(future, 0).into_iter().find(|key| !self.cache.contains(key) && !base.contains(key))));
            if let Some(key) = next
                && let Some(request) = &self.request
            {
                request.send(key)?;
                self.inflight = Some(key);
            }
        }
        if tracing {
            self.last_camera = Some((camera, self.frame));
        }
        self.compositor.render_submit(camera.origin(), camera.zoom, self.clear)?;
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
        // Presentation is asynchronous; a per-frame GPU fence would serialize the swapchain
        // and turn ordinary FIFO backpressure into accumulated input latency.
        self.device.poll(wgpu::PollType::Poll)?;
        self.occluded_since = None;
        if !self.first_present {
            self.first_present_ms = Some(self.boot_started.elapsed().as_secs_f64() * 1000.0);
            if let Some(signal) = self.warm_start_signal.take() {
                self.warm_started = Some(Instant::now());
                signal.send(())?;
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
        self.zoom_phase.push(self.frame >= 60);
        self.frame += 1;
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
            "cache slots={} atlas_mib={:.1} warm_ms={:.1} stress={} stress_gap_ms={} stress_jobs={} predict_steps={} resident={} evictions={} forced_evictions={} completed_jobs={} discarded_jobs={} job_p95_ms={:.2} missing_base_frames={} stale_detail_frames={} missing_base_tile_frames={} missing_detail_tile_frames={} queue_done_samples={} queue_done_p95_ms={:.2} queue_done_over_50ms={}",
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
            self.missing_base_tiles,
            self.missing_detail_tiles,
            queue_done.len(),
            p95(&queue_done),
            queue_done.iter().filter(|&&ms| ms > 50.0).count(),
        );
    }
}

#[derive(Default)]
struct App {
    state: Option<State>,
    failed: bool,
    next_watchdog: Option<Instant>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop) {
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
        if state.first_present && (!state.warm_in_background || state.warm_remaining.is_empty()) {
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
    let mut app = App::default();
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
    fn warm_plan_prioritizes_visible_tiles_and_keeps_zoom_tier_unique() {
        let camera = Camera { center: (1600.0, 1150.0), zoom: SCALE };
        let plan = warm_plan(camera, 160);
        let visible_base = visible(camera, 0);
        assert!(plan.len() > visible_base.len());
        assert!(plan.iter().take(visible_base.len()).all(|key| visible_base.contains(key)));
        assert_eq!(plan.iter().copied().collect::<HashSet<_>>().len(), plan.len());
        assert!(visible(camera, 1).iter().all(|key| plan.contains(key)));
    }

    #[test]
    fn overview_covers_the_initial_viewport_from_its_central_pixels() {
        let camera = Camera { center: (1600.0, 1150.0), zoom: SCALE };
        let bleed = 102;
        let view = preview_view(camera, bleed);
        let origin = camera.origin();
        assert!((view.origin.0 + f64::from(bleed) / view.scale - origin.0).abs() < 1e-6);
        assert!((view.origin.1 + f64::from(bleed) / view.scale - origin.1).abs() < 1e-6);
        assert!((f64::from(TILE) / view.scale - f64::from(WIDTH) / SCALE).abs() < 1e-6);
        assert!(f64::from(TILE) / view.scale >= f64::from(HEIGHT) / SCALE);
    }
}
