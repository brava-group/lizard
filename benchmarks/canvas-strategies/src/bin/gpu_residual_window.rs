//! Real Metal-swapchain pan/zoom trace for the sharp geometry + effect residual candidate.

use std::error::Error;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use canvas_strategies_bench::gpu_residual::GpuResidual;
use canvas_strategies_bench::residual::Residual;
use lizard_doc::EffectKind;
use lizard_render::{Options, View, render_view};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowId};

const WIDTH: u32 = 5120;
const HEIGHT: u32 = 2160;
const DEFAULT_FACTOR: u32 = 5;
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

fn source_view(cameras: &[Camera], factor: u32) -> Result<View, Box<dyn Error>> {
    if cameras.is_empty() {
        return Err("trace is empty".into());
    }
    let mut bounds = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for camera in cameras {
        let view = camera.view();
        bounds.0 = bounds.0.min(view.origin.0);
        bounds.1 = bounds.1.min(view.origin.1);
        bounds.2 = bounds.2.max(view.origin.0 + f64::from(WIDTH) / view.scale);
        bounds.3 = bounds.3.max(view.origin.1 + f64::from(HEIGHT) / view.scale);
    }
    let guard = 96.0;
    let scale = cameras.iter().map(|camera| camera.zoom).fold(0.0_f64, f64::max) + 0.02;
    let origin = (bounds.0 - guard, bounds.1 - guard);
    let width = (((bounds.2 + guard - origin.0) * scale).ceil() as u32).next_multiple_of(factor);
    let height = (((bounds.3 + guard - origin.1) * scale).ceil() as u32).next_multiple_of(factor);
    Ok(View { origin, scale, width, height })
}

struct Ready {
    canvas: GpuResidual,
    source: View,
    cpu_ms: f64,
    bake_ms: f64,
    upload_ms: f64,
}

fn prepare(
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: View,
    factor: u32,
    baked: bool,
    surface_format: wgpu::TextureFormat,
) -> Result<Ready, Box<dyn Error>> {
    let started = Instant::now();
    let (doc, warnings) = lizard_format::load_file(Path::new("benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard"))?;
    if !warnings.is_empty() {
        return Err(format!("fixture warnings: {warnings:?}").into());
    }
    let page = doc.current_page.ok_or("page missing")?;
    let low = View { origin: source.origin, scale: source.scale / f64::from(factor), width: source.width / factor, height: source.height / factor };
    let suppressed: Vec<_> = doc
        .nodes()
        .filter(|node| node.effects.iter().any(|effect| effect.visible && matches!(effect.kind, EffectKind::LayerBlur(_))))
        .map(|node| node.id)
        .collect();
    let opts = Options { lod: true, skip_effects: true, skip: suppressed, ..Default::default() };
    let low_full = render_view(&doc, page, &low, &Options { lod: true, ..Default::default() }).ok_or("low full render failed")?;
    let low_base = render_view(&doc, page, &low, &opts).ok_or("low base render failed")?;
    let mut high_base = render_view(&doc, page, &source, &opts).ok_or("sharp overscan render failed")?;
    let bake_started = Instant::now();
    if baked {
        let residual = Residual::new(low_full.clone(), low_base.clone(), low.clone()).ok_or("low views differ")?;
        residual.apply(&mut high_base, &source);
    }
    let bake_ms = if baked { bake_started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
    let cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let canvas = GpuResidual::new(device, queue, source.width, source.height, low.width, low.height, WIDTH, HEIGHT, baked, Some(surface_format));
    canvas.upload_images(&high_base, &low_full, &low_base)?;
    let upload_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(Ready { canvas, source, cpu_ms, bake_ms, upload_ms })
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    ready: Receiver<Result<Ready, String>>,
    canvas: Option<GpuResidual>,
    source: Option<View>,
    cameras: Vec<Camera>,
    blit: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
    boot: Instant,
    started: Option<Instant>,
    first_present_ms: Option<f64>,
    visual_ready_ms: Option<f64>,
    cpu_prepare_ms: Option<f64>,
    bake_ms: Option<f64>,
    upload_ms: Option<f64>,
    focus_delay: Duration,
    frame: usize,
    skipped: usize,
    present_ms: Vec<f64>,
    lateness_ms: Vec<f64>,
    gpu_done: Arc<Mutex<Vec<Option<f64>>>>,
    occluded_since: Option<Instant>,
    direct: bool,
}

impl State {
    fn new(event_loop: &ActiveEventLoop, proxy: EventLoopProxy<()>) -> Result<Self, Box<dyn Error>> {
        let boot = Instant::now();
        let window = Arc::new(event_loop.create_window(
            Window::default_attributes().with_title("Lizard sharp GPU residual benchmark").with_fullscreen(Some(Fullscreen::Borderless(None))),
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
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("residual present inputs"),
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("residual present shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("residual present pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("residual present pipeline"),
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
        let trace_repeats =
            std::env::args().find_map(|arg| arg.strip_prefix("--trace-repeats=").and_then(|value| value.parse::<usize>().ok())).unwrap_or(1);
        if !(1..=100).contains(&trace_repeats) {
            return Err("--trace-repeats must be between 1 and 100".into());
        }
        let cameras = trace().repeat(trace_repeats);
        let factor =
            std::env::args().find_map(|arg| arg.strip_prefix("--factor=").and_then(|value| value.parse::<u32>().ok())).unwrap_or(DEFAULT_FACTOR);
        if !(2..=6).contains(&factor) {
            return Err("--factor must be between 2 and 6".into());
        }
        let source = source_view(&cameras, factor)?;
        let baked = std::env::args().any(|arg| arg == "--baked");
        let direct = std::env::args().any(|arg| arg == "--direct");
        let focus_delay_ms = std::env::args()
            .find_map(|arg| arg.strip_prefix("--focus-delay-ms=").and_then(|value| value.parse::<u64>().ok()))
            .unwrap_or(0)
            .min(10_000);
        if source.width.div_ceil(2) + 2 > device.limits().max_texture_dimension_2d || source.height > device.limits().max_texture_dimension_2d {
            return Err("overscan texture exceeds device limit".into());
        }
        let (tx, ready) = mpsc::channel();
        let worker_device = device.clone();
        let worker_queue = queue.clone();
        let worker_source = source.clone();
        let surface_format = config.format;
        thread::spawn(move || {
            let result = prepare(worker_device, worker_queue, worker_source, factor, baked, surface_format).map_err(|error| error.to_string());
            let _ = tx.send(result);
            let _ = proxy.send_event(());
        });
        println!(
            "window={}x{} display_scale={} present_mode={:?} source={}x{} source_scale={:.4} adapter={} baked={baked} direct={direct} factor={factor} trace_repeats={trace_repeats} focus_delay_ms={focus_delay_ms}",
            size.width,
            size.height,
            window.scale_factor(),
            config.present_mode,
            source.width,
            source.height,
            source.scale,
            adapter.get_info().name,
        );
        let camera_count = cameras.len();
        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            ready,
            canvas: None,
            source: None,
            cameras,
            blit,
            bind_layout,
            sampler,
            bind: None,
            boot,
            started: None,
            first_present_ms: None,
            visual_ready_ms: None,
            cpu_prepare_ms: None,
            bake_ms: None,
            upload_ms: None,
            focus_delay: Duration::from_millis(focus_delay_ms),
            frame: 0,
            skipped: 0,
            present_ms: Vec::new(),
            lateness_ms: Vec::new(),
            gpu_done: Arc::new(Mutex::new(vec![None; camera_count])),
            occluded_since: None,
            direct,
        })
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        if let Ok(result) = self.ready.try_recv() {
            let ready = result.map_err(|error| -> Box<dyn Error> { error.into() })?;
            self.cpu_prepare_ms = Some(ready.cpu_ms);
            self.bake_ms = Some(ready.bake_ms);
            self.upload_ms = Some(ready.upload_ms);
            self.source = Some(ready.source);
            self.bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("residual present bind group"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(ready.canvas.output_view()) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            }));
            self.canvas = Some(ready.canvas);
            self.started = Some(Instant::now() + self.focus_delay);
        }
        let trace = self.started.filter(|started| Instant::now() >= *started).zip(self.source.as_ref()).zip(self.canvas.as_ref());
        let mut due = None;
        let mut camera = None;
        let start = Instant::now();
        if trace.is_none()
            && let (Some(source), Some(canvas)) = (self.source.as_ref(), self.canvas.as_ref())
        {
            if self.direct {
                camera = Some(self.cameras[0].view());
            } else {
                canvas.render_submit(source, &self.cameras[0].view());
            }
        }
        if let Some(((started, source), canvas)) = trace {
            let latest = (started.elapsed().as_secs_f64() * HZ).floor() as usize;
            if latest > self.frame {
                self.skipped += latest - self.frame;
                self.frame = latest.min(self.cameras.len().saturating_sub(1));
            }
            let scheduled = started + Duration::from_secs_f64(self.frame as f64 / HZ);
            if let Some(wait) = scheduled.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
            let view = self.cameras[self.frame].view();
            if self.direct {
                camera = Some(view);
            } else {
                canvas.render_submit(source, &view);
            }
            due = Some(scheduled);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.focus_window();
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
        if self.direct
            && let (Some(canvas), Some(source), Some(camera)) = (&self.canvas, &self.source, camera)
        {
            canvas.render_submit_to_surface(source, &camera, &view)?;
        } else {
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("residual present") });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("residual swapchain pass"),
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
                if let Some(bind) = &self.bind {
                    pass.set_pipeline(&self.blit);
                    pass.set_bind_group(0, bind, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
            self.queue.submit([encoder.finish()]);
        }
        if let Some(due) = due {
            let samples = Arc::clone(&self.gpu_done);
            let index = self.frame;
            self.queue.on_submitted_work_done(move || {
                if let Ok(mut samples) = samples.lock() {
                    samples[index] = Some(due.elapsed().as_secs_f64() * 1000.0);
                }
            });
        }
        self.queue.present(frame);
        self.device.poll(wgpu::PollType::Poll)?;
        self.occluded_since = None;
        if self.first_present_ms.is_none() {
            self.first_present_ms = Some(self.boot.elapsed().as_secs_f64() * 1000.0);
        }
        if self.bind.is_some() && self.visual_ready_ms.is_none() {
            self.visual_ready_ms = Some(self.boot.elapsed().as_secs_f64() * 1000.0);
        }
        if let Some(due) = due {
            self.present_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            self.lateness_ms.push(start.saturating_duration_since(due).as_secs_f64() * 1000.0);
            self.frame += 1;
        }
        Ok(())
    }

    fn report(&self) -> Result<(), Box<dyn Error>> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        let mut present = self.present_ms.clone();
        present.sort_by(f64::total_cmp);
        let mut completed =
            self.gpu_done.lock().map_err(|_| "GPU completion samples poisoned")?.iter().filter_map(|sample| *sample).collect::<Vec<_>>();
        completed.sort_by(f64::total_cmp);
        let p95 = |values: &[f64]| values.get((values.len() * 95 / 100).min(values.len().saturating_sub(1))).copied().unwrap_or(0.0);
        println!(
            "startup first_present_ms={:.1} visual_ready_ms={:.1} cpu_prepare_ms={:.1} bake_ms={:.1} upload_ms={:.1}",
            self.first_present_ms.unwrap_or(0.0),
            self.visual_ready_ms.unwrap_or(0.0),
            self.cpu_prepare_ms.unwrap_or(0.0),
            self.bake_ms.unwrap_or(0.0),
            self.upload_ms.unwrap_or(0.0)
        );
        println!(
            "trace presented={}/{} skipped={} present_median_ms={:.2} present_p95_ms={:.2} present_max_ms={:.2} present_over_50={} queue_done_p95_ms={:.2} queue_done_over_50={} max_schedule_lateness_ms={:.2}",
            present.len(),
            self.cameras.len(),
            self.skipped,
            present.get(present.len() / 2).copied().unwrap_or(0.0),
            p95(&present),
            present.last().copied().unwrap_or(0.0),
            present.iter().filter(|&&ms| ms > 50.0).count(),
            p95(&completed),
            completed.iter().filter(|&&ms| ms > 50.0).count(),
            self.lateness_ms.iter().copied().fold(0.0_f64, f64::max)
        );
        Ok(())
    }
}

struct App {
    state: Option<State>,
    proxy: EventLoopProxy<()>,
    failed: bool,
    deadline: Instant,
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
            }
            Err(error) => {
                eprintln!("residual window setup: {error}");
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
                let _ = state.report();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = state.draw() {
                    eprintln!("residual window frame: {error}");
                    self.failed = true;
                    event_loop.exit();
                } else if state.frame >= state.cameras.len() {
                    if let Err(error) = state.report() {
                        eprintln!("residual window report: {error}");
                        self.failed = true;
                    }
                    event_loop.exit();
                } else if state.started.is_some_and(|started| Instant::now() >= started) {
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
        if let Some(state) = &self.state
            && state.frame == 0
            && Instant::now() >= self.deadline
        {
            state.window.request_redraw();
            self.deadline = Instant::now() + Duration::from_millis(100);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = App { state: None, proxy: event_loop.create_proxy(), failed: false, deadline: Instant::now() + Duration::from_millis(100) };
    event_loop.run_app(&mut app)?;
    if app.failed {
        return Err("residual window benchmark failed".into());
    }
    Ok(())
}
