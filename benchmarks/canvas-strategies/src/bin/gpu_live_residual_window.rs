//! Real swapchain pan/zoom of live Vello vectors with document-wide cached soft effects.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use canvas_strategies_bench::geometry_atlas::{self, GeometryAtlas};
use canvas_strategies_bench::gpu_live_residual::GpuLiveResidual;
use canvas_strategies_bench::gpu_residual::GpuResidual;
use canvas_strategies_bench::gpu_scene::{GpuCanvas, make_scene_with_images};
use lizard_doc::{EffectKind, NodeKind};
use lizard_render::{Options, Pixmap, View, render_view};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Window, WindowId};

const WIDTH: u32 = 5120;
const HEIGHT: u32 = 2160;
const HZ: f64 = 75.0;
const BLIT: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = array<vec2<f32>, 6>(vec2<f32>(0.0,0.0),vec2<f32>(1.0,0.0),vec2<f32>(0.0,1.0),
                                  vec2<f32>(0.0,1.0),vec2<f32>(1.0,0.0),vec2<f32>(1.0,1.0))[i];
    var out: Out;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}
@fragment fn fs(input: Out) -> @location(0) vec4<f32> {
    return textureSample(source, linear_sampler, input.uv);
}
"#;

#[derive(Clone, Copy)]
struct Camera {
    center: (f64, f64),
    zoom: f64,
}

impl Camera {
    fn view(self) -> View {
        View {
            origin: (self.center.0 - f64::from(WIDTH) / self.zoom / 2.0, self.center.1 - f64::from(HEIGHT) / self.zoom / 2.0),
            scale: self.zoom,
            width: WIDTH,
            height: HEIGHT,
        }
    }
}

fn trace() -> Vec<Camera> {
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
        result.push(Camera { center, zoom: 2.0 });
    }
    let mut zoom = 2.0;
    let mut direction = 1.0;
    for i in 0..60 {
        if i % 8 == 0 {
            direction = -direction;
        }
        zoom = (zoom + direction * (0.012 + random() * 0.026)).clamp(2.0, 2.32);
        result.push(Camera { center, zoom });
    }
    result
}

fn excursion_trace() -> Vec<Camera> {
    let mut cameras = trace();
    for (index, camera) in cameras.iter_mut().enumerate() {
        let phase = index % 60;
        let progress = if phase < 30 { f64::from(phase as u32) / 30.0 } else { f64::from((60 - phase) as u32) / 30.0 };
        camera.center.0 += 1700.0 * progress;
        camera.center.1 += 500.0 * progress;
    }
    cameras
}

fn high_zoom_trace() -> Vec<Camera> {
    let mut cameras = trace();
    for (index, camera) in cameras.iter_mut().enumerate() {
        let progress = if index < 60 { index as f64 / 60.0 } else { (120 - index) as f64 / 60.0 };
        camera.zoom = 2.0 + 6.0 * progress;
    }
    cameras
}

fn source_contains(source: &View, camera: &View) -> bool {
    let source_right = source.origin.0 + f64::from(source.width) / source.scale;
    let source_bottom = source.origin.1 + f64::from(source.height) / source.scale;
    camera.origin.0 >= source.origin.0
        && camera.origin.1 >= source.origin.1
        && camera.origin.0 + f64::from(camera.width) / camera.scale <= source_right
        && camera.origin.1 + f64::from(camera.height) / camera.scale <= source_bottom
}

fn changed_percent(left: &[u8], right: &[u8], threshold: u8) -> f64 {
    let changed = left
        .chunks_exact(4)
        .zip(right.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(left, right)| left.abs_diff(*right) > threshold))
        .count();
    changed as f64 * 100.0 / f64::from(WIDTH * HEIGHT)
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    doc: lizard_doc::Document,
    page: lizard_doc::NodeId,
    clear: [f32; 3],
    gpu: GpuCanvas,
    residual: GpuLiveResidual,
    geometry_atlas: Option<GeometryAtlas>,
    atlas_one_pass: bool,
    atlas_faithful: bool,
    cached: Option<GpuResidual>,
    cached_source: Option<View>,
    cached_two_pass: bool,
    blit_pipeline: Option<wgpu::RenderPipeline>,
    blit_bind: Option<wgpu::BindGroup>,
    cached_frames: usize,
    live_frames: usize,
    atlas_frames: usize,
    atlas_first_ms: Option<f64>,
    atlas_by_repeat: Vec<usize>,
    presented_by_repeat: Vec<usize>,
    cameras: Vec<Camera>,
    started: Instant,
    setup_total_ms: f64,
    start_atlas_ready: bool,
    initial_margin: i32,
    atlas_upload_limit: usize,
    atlas_motion_margin: i32,
    warm_zoom_tiers: u8,
    motion_delay_ms: f64,
    warm_frame_interval_ms: u64,
    first_static_submitted: bool,
    initial_visual_ms: Option<f64>,
    initial_atlas_ready_ms: Option<f64>,
    warm_frames: usize,
    frame: usize,
    skipped: usize,
    submit: Vec<f64>,
    submit_atlas: Vec<f64>,
    submit_live: Vec<f64>,
    tile_phase: Vec<f64>,
    acquire_phase: Vec<f64>,
    draw_phase: Vec<f64>,
    present_phase: Vec<f64>,
    slow_frames: Vec<String>,
    scene: Vec<f64>,
    late: Vec<f64>,
    done: Arc<Mutex<Vec<Option<f64>>>>,
    poll_running: Option<Arc<AtomicBool>>,
    poll_thread: Option<JoinHandle<()>>,
}

impl State {
    fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        if std::env::var_os("LIZARD_GPU_NO_EFFECTS").is_none() {
            return Err("set LIZARD_GPU_NO_EFFECTS=1".into());
        }
        let attributes = Window::default_attributes().with_title("Lizard live vector GPU benchmark");
        let attributes = if std::env::args().any(|arg| arg == "--borderless-window") {
            attributes.with_inner_size(PhysicalSize::new(WIDTH, HEIGHT - 60)).with_decorations(false)
        } else {
            attributes.with_fullscreen(Some(Fullscreen::Borderless(None)))
        };
        let window = Arc::new(event_loop.create_window(attributes)?);
        window.set_visible(true);
        window.focus_window();
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).ok_or("no surface configuration")?;
        let requested_mode = if std::env::args().any(|arg| arg == "--present=mailbox") {
            wgpu::PresentMode::Mailbox
        } else if std::env::args().any(|arg| arg == "--present=immediate") {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };
        let capabilities = surface.get_capabilities(&adapter);
        if !capabilities.present_modes.contains(&requested_mode) {
            return Err(format!("present mode {requested_mode:?} unavailable; supported: {:?}", capabilities.present_modes).into());
        }
        config.present_mode = requested_mode;
        config.desired_maximum_frame_latency = std::env::args()
            .find_map(|arg| arg.strip_prefix("--frame-latency=").and_then(|value| value.parse::<u32>().ok()))
            .unwrap_or(config.desired_maximum_frame_latency)
            .clamp(1, 4);
        surface.configure(&device, &config);
        println!(
            "present_mode={:?} frame_latency={} supported_modes={:?}",
            config.present_mode, config.desired_maximum_frame_latency, capabilities.present_modes
        );
        let (doc, warnings) =
            lizard_format::load_file(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ultrawide-workbook.lizard"))?;
        if !warnings.is_empty() {
            return Err(format!("fixture warnings: {warnings:?}").into());
        }
        let page = doc.current_page.ok_or("page missing")?;
        let clear = match doc.node(page).map(|node| &node.kind) {
            Some(NodeKind::Page { background, .. }) => [background.r, background.g, background.b],
            _ => [1.0; 3],
        };
        let root = doc.node(page).ok_or("page node missing")?;
        let mut bounds = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &id in &root.children {
            if let Some(rect) = doc.world_render_bounds(id) {
                bounds.0 = bounds.0.min(rect.x0);
                bounds.1 = bounds.1.min(rect.y0);
                bounds.2 = bounds.2.max(rect.x1);
                bounds.3 = bounds.3.max(rect.y1);
            }
        }
        if !bounds.0.is_finite() || !bounds.2.is_finite() {
            return Err("document bounds missing".into());
        }
        let low_scale = 2.4 / 5.0;
        let low = View {
            origin: (bounds.0 - 128.0, bounds.1 - 128.0),
            scale: low_scale,
            width: (((bounds.2 - bounds.0) + 256.0) * low_scale).ceil() as u32,
            height: (((bounds.3 - bounds.1) + 256.0) * low_scale).ceil() as u32,
        };
        let setup = Instant::now();
        let low_full = render_view(&doc, page, &low, &Options { lod: true, ..Default::default() }).ok_or("low full render failed")?;
        let mut gpu = GpuCanvas::from_device(device.clone(), queue.clone(), adapter.get_info().name, WIDTH, HEIGHT);
        gpu.register_images(&doc)?;
        let mut low_gpu = GpuCanvas::shared_with(&gpu, low.width, low.height);
        low_gpu.register_images(&doc)?;
        let low_scene = make_scene_with_images(&doc, page, &low, Some(low_gpu.image_info()));
        low_gpu.render_submit_with_clear(&low_scene, clear)?;
        let low_pixels = low_gpu.pixels()?;
        let low_base = if std::env::args().any(|arg| arg == "--low-cpu-base") {
            render_view(&doc, page, &low, &Options { lod: true, skip_effects: true, ..Default::default() }).ok_or("low CPU geometry render failed")?
        } else {
            let mut low_base = Pixmap::new(low.width, low.height).ok_or("low geometry allocation failed")?;
            low_base.data_mut().copy_from_slice(&low_pixels);
            low_base
        };
        let mut residual = GpuLiveResidual::new(device.clone(), queue.clone(), gpu.output_view(), low.clone(), WIDTH, HEIGHT, Some(config.format));
        residual.upload(&low_full, &low_base)?;
        let trace_kind = std::env::args().find_map(|arg| arg.strip_prefix("--trace=").map(str::to_owned)).unwrap_or_else(|| "random".to_owned());
        let trace = match trace_kind.as_str() {
            "random" => trace(),
            "excursion" => excursion_trace(),
            "high-zoom" => high_zoom_trace(),
            _ => return Err(format!("unknown trace: {trace_kind}").into()),
        };
        let cameras = trace.repeat(
            std::env::args()
                .find_map(|arg| arg.strip_prefix("--trace-repeats=").and_then(|value| value.parse::<usize>().ok()))
                .unwrap_or(1)
                .clamp(1, 100),
        );
        if std::env::args().any(|arg| arg == "--geometry-atlas") {
            let first_cycle = cameras.iter().take(120).copied().map(Camera::view).collect::<Vec<_>>();
            println!(
                "atlas_trace_tiles_visible={:?} atlas_trace_tiles_margin1={:?}",
                geometry_atlas::distinct_tile_counts(&first_cycle, 0),
                geometry_atlas::distinct_tile_counts(&first_cycle, 1)
            );
        }
        let cache_guard =
            std::env::args().find_map(|arg| arg.strip_prefix("--cache-guard=").and_then(|value| value.parse::<f64>().ok())).unwrap_or(0.0);
        if !cache_guard.is_finite() || !(0.0..=2048.0).contains(&cache_guard) {
            return Err("--cache-guard must be finite and within 0..=2048".into());
        }
        let (cached, cached_source) = if cache_guard > 0.0 {
            let first = cameras.first().ok_or("empty camera trace")?.view();
            let scale = 2.4;
            let factor = 5;
            let source = View {
                origin: (first.origin.0 - cache_guard, first.origin.1 - cache_guard),
                scale,
                width: (((f64::from(WIDTH) / first.scale + cache_guard * 2.0) * scale).ceil() as u32).next_multiple_of(factor),
                height: (((f64::from(HEIGHT) / first.scale + cache_guard * 2.0) * scale).ceil() as u32).next_multiple_of(factor),
            };
            if source.width.div_ceil(2) + 2 > device.limits().max_texture_dimension_2d || source.height > device.limits().max_texture_dimension_2d {
                return Err("cached source exceeds texture limits".into());
            }
            let small =
                View { origin: source.origin, scale: source.scale / f64::from(factor), width: source.width / factor, height: source.height / factor };
            let suppressed = doc
                .nodes()
                .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
                .map(|node| node.id)
                .collect();
            let opts = Options { lod: true, skip_effects: true, skip: suppressed, ..Default::default() };
            let full = render_view(&doc, page, &small, &Options { lod: true, ..Default::default() }).ok_or("cached low full failed")?;
            let cache_gpu_geometry = std::env::args().any(|arg| arg == "--cache-gpu-geometry");
            let (base, high) = if cache_gpu_geometry {
                let started = Instant::now();
                let mut small_gpu = GpuCanvas::from_device(device.clone(), queue.clone(), adapter.get_info().name, small.width, small.height);
                small_gpu.register_images(&doc)?;
                let scene = make_scene_with_images(&doc, page, &small, Some(small_gpu.image_info()));
                small_gpu.render_submit_with_clear(&scene, clear)?;
                let pixels = small_gpu.pixels()?;
                let mut base = Pixmap::new(small.width, small.height).ok_or("cached low geometry allocation failed")?;
                base.data_mut().copy_from_slice(&pixels);
                let mut high = Pixmap::new(source.width, source.height).ok_or("cached high geometry allocation failed")?;
                let limit = device.limits().max_texture_dimension_2d;
                if source.height > limit {
                    return Err("cached high geometry exceeds device texture height".into());
                }
                let bands = source.width.div_ceil(limit);
                let band_width = source.width.div_ceil(bands);
                for band in 0..bands {
                    let start = band * band_width;
                    let end = (start + band_width).min(source.width);
                    let view = View {
                        origin: (source.origin.0 + f64::from(start) / source.scale, source.origin.1),
                        scale: source.scale,
                        width: end - start,
                        height: source.height,
                    };
                    let mut high_gpu = GpuCanvas::from_device(device.clone(), queue.clone(), adapter.get_info().name, view.width, view.height);
                    high_gpu.register_images(&doc)?;
                    let scene = make_scene_with_images(&doc, page, &view, Some(high_gpu.image_info()));
                    high_gpu.render_submit_with_clear(&scene, clear)?;
                    let pixels = high_gpu.pixels()?;
                    for row in 0..source.height as usize {
                        let from = row * view.width as usize * 4;
                        let to = (row * source.width as usize + start as usize) * 4;
                        high.data_mut()[to..to + view.width as usize * 4].copy_from_slice(&pixels[from..from + view.width as usize * 4]);
                    }
                }
                println!("cached_gpu_geometry_ms={:.1}", started.elapsed().as_secs_f64() * 1000.0);
                (base, high)
            } else {
                (
                    render_view(&doc, page, &small, &opts).ok_or("cached low base failed")?,
                    render_view(&doc, page, &source, &opts).ok_or("cached high geometry failed")?,
                )
            };
            let cache = GpuResidual::new(
                device.clone(),
                queue.clone(),
                source.width,
                source.height,
                small.width,
                small.height,
                WIDTH,
                HEIGHT,
                false,
                Some(config.format),
            );
            cache.upload_images(&high, &full, &base)?;
            println!("cached_source={}x{} guard={cache_guard:.0}", source.width, source.height);
            (Some(cache), Some(source))
        } else {
            (None, None)
        };
        let cached_two_pass = std::env::args().any(|arg| arg == "--cached-two-pass");
        let (blit_pipeline, blit_bind) = if cached_two_pass {
            let cache = cached.as_ref().ok_or("--cached-two-pass requires --cache-guard")?;
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("hybrid cached blit inputs"),
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
                label: Some("hybrid cached blit bind"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(cache.output_view()) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("hybrid cached blit shader"),
                source: wgpu::ShaderSource::Wgsl(BLIT.into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("hybrid cached blit layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hybrid cached blit pipeline"),
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
            (Some(pipeline), Some(bind))
        } else {
            (None, None)
        };
        let atlas_one_pass = std::env::args().any(|arg| arg == "--atlas-one-pass");
        let atlas_faithful = std::env::args().any(|arg| arg == "--atlas-faithful");
        if atlas_one_pass && atlas_faithful {
            return Err("--atlas-one-pass cannot be combined with --atlas-faithful".into());
        }
        let initial_margin = std::env::args()
            .find_map(|arg| arg.strip_prefix("--initial-margin=").and_then(|value| value.parse::<i32>().ok()))
            .unwrap_or(1)
            .clamp(0, 3);
        let motion_delay_ms = std::env::args()
            .find_map(|arg| arg.strip_prefix("--motion-delay-ms=").and_then(|value| value.parse::<f64>().ok()))
            .unwrap_or(0.0)
            .clamp(0.0, 10_000.0);
        let warm_frame_interval_ms = std::env::args()
            .find_map(|arg| arg.strip_prefix("--warm-frame-interval-ms=").and_then(|value| value.parse::<u64>().ok()))
            .unwrap_or(13)
            .clamp(13, 4_000);
        let atlas_slots = std::env::args()
            .find_map(|arg| arg.strip_prefix("--atlas-slots=").and_then(|value| value.parse::<u32>().ok()))
            .unwrap_or(geometry_atlas::DEFAULT_CAPACITY);
        let atlas_bleed = std::env::args()
            .find_map(|arg| arg.strip_prefix("--atlas-bleed=").and_then(|value| value.parse::<u32>().ok()))
            .unwrap_or(if atlas_faithful { 128 } else { 2 })
            .min(512);
        let atlas_upload_limit = std::env::args()
            .find_map(|arg| arg.strip_prefix("--atlas-upload-limit=").and_then(|value| value.parse::<usize>().ok()))
            .unwrap_or(2)
            .min(8);
        let atlas_motion_margin = std::env::args()
            .find_map(|arg| arg.strip_prefix("--atlas-motion-margin=").and_then(|value| value.parse::<i32>().ok()))
            .unwrap_or(1)
            .clamp(0, 2);
        let warm_zoom_tiers =
            std::env::args().find_map(|arg| arg.strip_prefix("--warm-zoom-tiers=").and_then(|value| value.parse::<u8>().ok())).unwrap_or(0).min(2);
        let geometry_atlas = if std::env::args().any(|arg| arg == "--geometry-atlas") {
            if cached.is_some() {
                return Err("--geometry-atlas cannot be combined with --cache-guard".into());
            }
            let initial_camera = cameras.first().ok_or("empty camera trace")?.view();
            let required = geometry_atlas::required_slots(&initial_camera, initial_margin);
            if usize::try_from(atlas_slots)? < required {
                return Err(
                    format!("--atlas-slots={atlas_slots} cannot retain the {required} tiles required by --initial-margin={initial_margin}").into()
                );
            }
            let mut atlas = GeometryAtlas::new(
                Arc::new(doc.clone()),
                page,
                device.clone(),
                queue.clone(),
                WIDTH,
                HEIGHT,
                atlas_slots,
                atlas_bleed,
                !atlas_faithful,
            )?;
            atlas.request_margin(&cameras.first().ok_or("empty camera trace")?.view(), initial_margin)?;
            if warm_zoom_tiers > 0 {
                atlas.prefetch_zoom_tiers(&initial_camera, warm_zoom_tiers);
            }
            if atlas_one_pass {
                let (full, base) = residual.low_views();
                atlas.enable_one_pass(config.format, &full, &base, &low)?;
            }
            if atlas_faithful {
                atlas.enable_surface(config.format)?;
            }
            residual.set_alt_sharp(atlas.output_view());
            Some(atlas)
        } else {
            if atlas_one_pass {
                return Err("--atlas-one-pass requires --geometry-atlas".into());
            }
            if atlas_faithful {
                return Err("--atlas-faithful requires --geometry-atlas".into());
            }
            None
        };
        if std::env::args().any(|arg| arg == "--compare-routes")
            && let (Some(cache), Some(source)) = (&cached, &cached_source)
        {
            for &index in &[0usize, 20, 40, 60, 80, 100, 119] {
                let Some(camera) = cameras.get(index).copied().map(Camera::view).filter(|camera| source_contains(source, camera)) else {
                    continue;
                };
                cache.render_submit(source, &camera);
                let cache_pixels = cache.pixels()?;
                let scene = make_scene_with_images(&doc, page, &camera, Some(gpu.image_info()));
                gpu.render_submit_with_clear(&scene, clear)?;
                if index == 0 {
                    let gpu_geometry = gpu.pixels()?;
                    let cpu_geometry = render_view(&doc, page, &camera, &Options { lod: true, skip_effects: true, ..Default::default() })
                        .ok_or("geometry comparison reference failed")?;
                    for threshold in [4, 16] {
                        println!(
                            "geometry_parity camera={index} threshold={threshold} gpu_cpu={:.3}%",
                            changed_percent(&gpu_geometry, cpu_geometry.data(), threshold)
                        );
                    }
                }
                residual.render_submit(&camera);
                let live_pixels = residual.pixels()?;
                let reference =
                    render_view(&doc, page, &camera, &Options { lod: true, ..Default::default() }).ok_or("route comparison reference failed")?;
                if index == 0 && std::env::args().any(|arg| arg == "--save-routes") {
                    for (name, pixels) in [("cached", &cache_pixels), ("live", &live_pixels)] {
                        let path = format!("target/canvas-strategies/hybrid-{name}.png");
                        let file = std::fs::File::create(path)?;
                        let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
                        encoder.set_color(png::ColorType::Rgba);
                        encoder.set_depth(png::BitDepth::Eight);
                        encoder.write_header()?.write_image_data(pixels)?;
                    }
                }
                for threshold in [4, 16] {
                    println!(
                        "route_parity camera={index} threshold={threshold} cache_live={:.3}% cache_reference={:.3}% live_reference={:.3}%",
                        changed_percent(&cache_pixels, &live_pixels, threshold),
                        changed_percent(&cache_pixels, reference.data(), threshold),
                        changed_percent(&live_pixels, reference.data(), threshold),
                    );
                }
            }
        }
        let start_atlas_ready = std::env::args().any(|arg| arg == "--start-atlas-ready");
        if start_atlas_ready && geometry_atlas.is_none() {
            return Err("--start-atlas-ready requires --geometry-atlas".into());
        }
        let setup_total_ms = setup.elapsed().as_secs_f64() * 1000.0;
        println!(
            "window={}x{} display_scale={} adapter={} low={}x{} setup_ms={:.1}",
            size.width,
            size.height,
            window.scale_factor(),
            adapter.get_info().name,
            low.width,
            low.height,
            setup_total_ms
        );
        let count = cameras.len();
        let (poll_running, poll_thread) = if std::env::args().any(|arg| arg == "--poll-background") {
            let running = Arc::new(AtomicBool::new(true));
            let thread_running = Arc::clone(&running);
            let thread_device = device.clone();
            let thread = std::thread::spawn(move || {
                while thread_running.load(Ordering::Acquire) {
                    let _ = thread_device.poll(wgpu::PollType::Poll);
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            (Some(running), Some(thread))
        } else {
            (None, None)
        };
        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            doc,
            page,
            clear,
            gpu,
            residual,
            geometry_atlas,
            atlas_one_pass,
            atlas_faithful,
            cached,
            cached_source,
            cached_two_pass,
            blit_pipeline,
            blit_bind,
            cached_frames: 0,
            live_frames: 0,
            atlas_frames: 0,
            atlas_first_ms: None,
            atlas_by_repeat: vec![0; count.div_ceil(120)],
            presented_by_repeat: vec![0; count.div_ceil(120)],
            cameras,
            started: Instant::now(),
            setup_total_ms,
            start_atlas_ready,
            initial_margin,
            atlas_upload_limit,
            atlas_motion_margin,
            warm_zoom_tiers,
            motion_delay_ms,
            warm_frame_interval_ms,
            first_static_submitted: false,
            initial_visual_ms: None,
            initial_atlas_ready_ms: None,
            warm_frames: 0,
            frame: 0,
            skipped: 0,
            submit: Vec::new(),
            submit_atlas: Vec::new(),
            submit_live: Vec::new(),
            tile_phase: Vec::new(),
            acquire_phase: Vec::new(),
            draw_phase: Vec::new(),
            present_phase: Vec::new(),
            slow_frames: Vec::new(),
            scene: Vec::new(),
            late: Vec::new(),
            done: Arc::new(Mutex::new(vec![None; count])),
            poll_running,
            poll_thread,
        })
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        if self.start_atlas_ready {
            if self.started.elapsed() > Duration::from_secs(30) {
                return Err("geometry atlas did not become ready within 30 seconds".into());
            }
            let camera = self.cameras.first().ok_or("empty camera trace")?.view();
            let atlas = self.geometry_atlas.as_mut().ok_or("geometry atlas unavailable")?;
            atlas.install(&camera, 2)?;
            atlas.request_margin(&camera, self.initial_margin)?;
            if self.warm_zoom_tiers > 0 {
                atlas.prefetch_zoom_tiers(&camera, self.warm_zoom_tiers);
            }
            if !self.first_static_submitted {
                let scene = make_scene_with_images(&self.doc, self.page, &camera, Some(self.gpu.image_info()));
                self.gpu.render_submit_with_clear(&scene, self.clear)?;
                let frame = match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    wgpu::CurrentSurfaceTexture::Occluded => {
                        self.window.focus_window();
                        return Ok(());
                    }
                    wgpu::CurrentSurfaceTexture::Timeout => return Ok(()),
                    _ => {
                        self.surface.configure(&self.device, &self.config);
                        return Ok(());
                    }
                };
                let view = frame.texture.create_view(&Default::default());
                self.residual.render_submit_to_surface(&camera, &view)?;
                self.queue.present(frame);
                self.first_static_submitted = true;
                self.initial_visual_ms = Some(self.setup_total_ms + self.started.elapsed().as_secs_f64() * 1000.0);
            }
            if atlas.covers_margin(&camera, self.initial_margin) {
                if self.initial_atlas_ready_ms.is_none() {
                    self.initial_atlas_ready_ms = Some(self.setup_total_ms + self.started.elapsed().as_secs_f64() * 1000.0);
                    println!(
                        "initial_static_visual_ms={:?} initial_atlas_ready_ms={:?} initial_atlas_stats={:?}",
                        self.initial_visual_ms,
                        self.initial_atlas_ready_ms,
                        atlas.stats()
                    );
                }
                if self.setup_total_ms + self.started.elapsed().as_secs_f64() * 1000.0 >= self.motion_delay_ms {
                    println!(
                        "warm_static_frames={} zoom_prefetch_ready={} atlas_stats={:?}",
                        self.warm_frames,
                        atlas.prefetched_zoom_tiers_ready(&camera, self.warm_zoom_tiers),
                        atlas.stats()
                    );
                    self.started = Instant::now();
                    self.start_atlas_ready = false;
                } else {
                    let frame = match self.surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
                        wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => None,
                        _ => {
                            self.surface.configure(&self.device, &self.config);
                            None
                        }
                    };
                    if let Some(frame) = frame {
                        let view = frame.texture.create_view(&Default::default());
                        if self.atlas_faithful {
                            atlas.render_to_surface(&camera, self.clear, &view)?;
                        } else if self.atlas_one_pass {
                            atlas.render_one_pass_to_surface(&camera, self.clear, &view)?;
                        } else {
                            atlas.render_submit(&camera, self.clear)?;
                            self.residual.render_submit_alt_to_surface(&camera, &view)?;
                        }
                        self.queue.present(frame);
                        self.warm_frames += 1;
                    }
                    let waiting_for_zoom = self.warm_zoom_tiers > 0 && !atlas.prefetched_zoom_tiers_ready(&camera, self.warm_zoom_tiers);
                    std::thread::sleep(Duration::from_millis(if waiting_for_zoom { 13 } else { self.warm_frame_interval_ms }));
                }
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
            return Ok(());
        }
        let latest = (self.started.elapsed().as_secs_f64() * HZ).floor() as usize;
        if latest > self.frame {
            self.skipped += latest - self.frame;
            self.frame = latest.min(self.cameras.len().saturating_sub(1));
        }
        let due = self.started + Duration::from_secs_f64(self.frame as f64 / HZ);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let camera = self.cameras[self.frame].view();
        let started = Instant::now();
        let (atlas_hit, tile_uploads) = if let Some(atlas) = self.geometry_atlas.as_mut() {
            atlas.request_margin(&camera, self.atlas_motion_margin)?;
            let installed = atlas.install(&camera, self.atlas_upload_limit)?;
            (atlas.covers(&camera), installed)
        } else {
            (false, 0)
        };
        let after_tiles = Instant::now();
        let cache_hit = self.cached_source.as_ref().is_some_and(|source| source_contains(source, &camera));
        if !cache_hit && !atlas_hit {
            let scene = make_scene_with_images(&self.doc, self.page, &camera, Some(self.gpu.image_info()));
            self.scene.push(started.elapsed().as_secs_f64() * 1000.0);
            self.gpu.render_submit_with_clear(&scene, self.clear)?;
        }
        let after_scene = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.focus_window();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Err("surface acquisition timed out".into()),
            _ => return Err("surface lost".into()),
        };
        let view = frame.texture.create_view(&Default::default());
        let after_acquire = Instant::now();
        if atlas_hit {
            let atlas = self.geometry_atlas.as_ref().ok_or("geometry atlas unavailable")?;
            if self.atlas_faithful {
                atlas.render_to_surface(&camera, self.clear, &view)?;
            } else if self.atlas_one_pass {
                atlas.render_one_pass_to_surface(&camera, self.clear, &view)?;
            } else {
                atlas.render_submit(&camera, self.clear)?;
                self.residual.render_submit_alt_to_surface(&camera, &view)?;
            }
            self.atlas_frames += 1;
            self.atlas_first_ms.get_or_insert_with(|| self.started.elapsed().as_secs_f64() * 1000.0);
            self.atlas_by_repeat[self.frame / 120] += 1;
        } else if cache_hit {
            if let (Some(cache), Some(source)) = (&self.cached, &self.cached_source) {
                if self.cached_two_pass {
                    cache.render_submit(source, &camera);
                    let pipeline = self.blit_pipeline.as_ref().ok_or("cached blit pipeline unavailable")?;
                    let bind = self.blit_bind.as_ref().ok_or("cached blit bind unavailable")?;
                    let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("hybrid cached present") });
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("hybrid cached present"),
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
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, bind, &[]);
                        pass.draw(0..6, 0..1);
                    }
                    self.queue.submit([encoder.finish()]);
                } else {
                    cache.render_submit_to_surface(source, &camera, &view)?;
                }
                self.cached_frames += 1;
            }
        } else {
            self.residual.render_submit_to_surface(&camera, &view)?;
            self.live_frames += 1;
        }
        let after_draw = Instant::now();
        let samples = Arc::clone(&self.done);
        let index = self.frame;
        self.queue.on_submitted_work_done(move || {
            if let Ok(mut values) = samples.lock() {
                values[index] = Some(due.elapsed().as_secs_f64() * 1000.0);
            }
        });
        self.queue.present(frame);
        let after_present = Instant::now();
        if self.poll_running.is_none() {
            self.device.poll(wgpu::PollType::Poll)?;
        }
        let after_poll = Instant::now();
        let submit_ms = after_poll.duration_since(started).as_secs_f64() * 1000.0;
        self.submit.push(submit_ms);
        self.tile_phase.push(after_tiles.duration_since(started).as_secs_f64() * 1000.0);
        self.acquire_phase.push(after_acquire.duration_since(after_scene).as_secs_f64() * 1000.0);
        self.draw_phase.push(after_draw.duration_since(after_acquire).as_secs_f64() * 1000.0);
        self.present_phase.push(after_present.duration_since(after_draw).as_secs_f64() * 1000.0);
        if atlas_hit {
            self.submit_atlas.push(submit_ms);
        } else if self.geometry_atlas.is_some() {
            self.submit_live.push(submit_ms);
        }
        if submit_ms > 50.0 && self.slow_frames.len() < 12 {
            self.slow_frames.push(format!(
                "frame={} atlas={} scheduled={} total={submit_ms:.2} tile={:.2} scene={:.2} acquire={:.2} draw={:.2} present={:.2} poll={:.2}",
                self.frame,
                atlas_hit,
                tile_uploads,
                after_tiles.duration_since(started).as_secs_f64() * 1000.0,
                after_scene.duration_since(after_tiles).as_secs_f64() * 1000.0,
                after_acquire.duration_since(after_scene).as_secs_f64() * 1000.0,
                after_draw.duration_since(after_acquire).as_secs_f64() * 1000.0,
                after_present.duration_since(after_draw).as_secs_f64() * 1000.0,
                after_poll.duration_since(after_present).as_secs_f64() * 1000.0,
            ));
        }
        self.presented_by_repeat[self.frame / 120] += 1;
        self.late.push(started.saturating_duration_since(due).as_secs_f64() * 1000.0);
        self.frame += 1;
        Ok(())
    }

    fn report(&mut self) -> Result<(), Box<dyn Error>> {
        self.stop_poller()?;
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        let mut submit = self.submit.clone();
        let mut scene = self.scene.clone();
        let mut done = self.done.lock().map_err(|_| "completion samples poisoned")?.iter().filter_map(|sample| *sample).collect::<Vec<_>>();
        submit.sort_by(f64::total_cmp);
        scene.sort_by(f64::total_cmp);
        done.sort_by(f64::total_cmp);
        let p95 = |v: &[f64]| v.get((v.len() * 95 / 100).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0);
        println!(
            "trace presented={}/{} skipped={} cached_frames={} atlas_frames={} live_frames={} scene_p95_ms={:.2} submit_p95_ms={:.2} submit_max_ms={:.2} submit_over_50={} queue_done_p95_ms={:.2} queue_done_over_50={} max_late_ms={:.2} atlas_first_ms={:?} atlas_stats={:?}",
            submit.len(),
            self.cameras.len(),
            self.skipped,
            self.cached_frames,
            self.atlas_frames,
            self.live_frames,
            p95(&scene),
            p95(&submit),
            submit.last().copied().unwrap_or(0.0),
            submit.iter().filter(|&&x| x > 50.0).count(),
            p95(&done),
            done.iter().filter(|&&x| x > 50.0).count(),
            self.late.iter().copied().fold(0.0_f64, f64::max),
            self.atlas_first_ms,
            self.geometry_atlas.as_ref().map(GeometryAtlas::stats)
        );
        if self.geometry_atlas.is_some() {
            for (name, times) in
                [("tile", &self.tile_phase), ("acquire", &self.acquire_phase), ("draw", &self.draw_phase), ("present", &self.present_phase)]
            {
                let mut sorted = times.clone();
                sorted.sort_by(f64::total_cmp);
                println!("phase={name} p95_ms={:.2} max_ms={:.2}", p95(&sorted), sorted.last().copied().unwrap_or(0.0));
            }
            let mut atlas_times = self.submit_atlas.clone();
            let mut live_times = self.submit_live.clone();
            atlas_times.sort_by(f64::total_cmp);
            live_times.sort_by(f64::total_cmp);
            println!(
                "atlas_per_repeat={:?} presented_per_repeat={:?} atlas_submit_p95_ms={:.2} live_submit_p95_ms={:.2} slow_frames_first={:?}",
                self.atlas_by_repeat,
                self.presented_by_repeat,
                p95(&atlas_times),
                p95(&live_times),
                self.slow_frames
            );
        }
        if std::env::args().any(|arg| arg == "--verify-atlas")
            && let Some(atlas) = &mut self.geometry_atlas
        {
            for &index in &[0usize, 20, 40, 60, 80, 100, 119] {
                let Some(camera) = self.cameras.get(index).copied().map(Camera::view) else { continue };
                atlas.request_margin(&camera, 0)?;
                atlas.install(&camera, 0)?;
                if !atlas.covers(&camera) {
                    println!("atlas_parity camera={index} missing_tiles");
                    continue;
                }
                let actual = if self.atlas_faithful {
                    atlas.render_submit(&camera, self.clear)?;
                    atlas.pixels()?
                } else if self.atlas_one_pass {
                    atlas.render_one_pass(&camera, self.clear)?
                } else {
                    atlas.render_submit(&camera, self.clear)?;
                    self.residual.render_submit_alt(&camera)?;
                    self.residual.pixels()?
                };
                let reference = render_view(&self.doc, self.page, &camera, &Options { lod: true, ..Default::default() })
                    .ok_or("atlas reference render failed")?;
                for threshold in [4, 16] {
                    println!(
                        "atlas_parity camera={index} threshold={threshold} changed={:.3}%",
                        changed_percent(&actual, reference.data(), threshold)
                    );
                }
                if (index == 0 || index == 60) && std::env::args().any(|arg| arg == "--save-atlas") {
                    for (name, pixels) in [("actual", actual.as_slice()), ("reference", reference.data())] {
                        let prefix = if index == 60 { "high-zoom-atlas" } else { "atlas" };
                        let file = std::fs::File::create(format!("target/canvas-strategies/{prefix}-{name}.png"))?;
                        let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
                        encoder.set_color(png::ColorType::Rgba);
                        encoder.set_depth(png::BitDepth::Eight);
                        encoder.write_header()?.write_image_data(pixels)?;
                    }
                }
            }
        }
        if std::env::args().any(|arg| arg == "--verify-live") {
            for &index in &[0usize, 30, 60, 90, 119] {
                let Some(camera) = self.cameras.get(index).copied().map(Camera::view) else { continue };
                let scene = make_scene_with_images(&self.doc, self.page, &camera, Some(self.gpu.image_info()));
                self.gpu.render_submit_with_clear(&scene, self.clear)?;
                self.residual.render_submit(&camera);
                let actual = self.residual.pixels()?;
                let reference = render_view(&self.doc, self.page, &camera, &Options { lod: true, ..Default::default() })
                    .ok_or("live route reference render failed")?;
                for threshold in [4, 16] {
                    println!(
                        "live_parity camera={index} zoom={:.2} threshold={threshold} changed={:.3}%",
                        camera.scale,
                        changed_percent(&actual, reference.data(), threshold)
                    );
                }
                if index == 60 && std::env::args().any(|arg| arg == "--save-live") {
                    for (name, pixels) in [("actual", actual.as_slice()), ("reference", reference.data())] {
                        let file = std::fs::File::create(format!("target/canvas-strategies/high-zoom-live-{name}.png"))?;
                        let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
                        encoder.set_color(png::ColorType::Rgba);
                        encoder.set_depth(png::BitDepth::Eight);
                        encoder.write_header()?.write_image_data(pixels)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn stop_poller(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(running) = self.poll_running.take() {
            running.store(false, Ordering::Release);
        }
        if let Some(thread) = self.poll_thread.take() {
            thread.join().map_err(|_| "GPU poll thread panicked")?;
        }
        Ok(())
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = self.stop_poller();
    }
}

struct App {
    state: Option<State>,
    failed: bool,
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
            }
            Err(error) => {
                eprintln!("live vector setup: {error}");
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
                let _ = state.report();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = state.draw() {
                    eprintln!("live vector frame: {error}");
                    self.failed = true;
                    event_loop.exit();
                } else if state.frame >= state.cameras.len() {
                    if let Err(error) = state.report() {
                        eprintln!("live vector report: {error}");
                        self.failed = true;
                    }
                    event_loop.exit();
                } else {
                    state.window.request_redraw();
                }
            }
            WindowEvent::Resized(size)
                if size.width > 0 && size.height > 0 && (size.width != state.config.width || size.height != state.config.height) =>
            {
                state.config.width = size.width;
                state.config.height = size.height;
                state.surface.configure(&state.device, &state.config);
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(13)));
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = App { state: None, failed: false };
    event_loop.run_app(&mut app)?;
    if app.failed {
        return Err("live vector window benchmark failed".into());
    }
    Ok(())
}
