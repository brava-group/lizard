//! Bounded native WebGPU pass. The CPU scene renderer composites these pixels into a path.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use lizard_doc::{ShaderDefinition, ShaderParameterControl};

use crate::{MAX_DIMENSION, MAX_PARAMETERS, MAX_PIXELS, PRELUDE, validate};

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: HashMap<String, wgpu::RenderPipeline>,
}

impl Gpu {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| format!("no GPU adapter for shader fills: {e}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("shader GPU device failed: {e}"))?;
        Ok(Self { device, queue, pipelines: HashMap::new() })
    }

    fn pipeline(&mut self, def: &ShaderDefinition) -> Result<&wgpu::RenderPipeline, String> {
        let key = format!("{}\n{}", def.entry_point, def.source);
        if !self.pipelines.contains_key(&key) {
            validate(def)?;
            let source = format!("{PRELUDE}\n{}", def.source);
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Lizard shader fill"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Lizard shader globals"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                }],
            });
            let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Lizard shader fill layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Lizard shader fill pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("shader_vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(&def.entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            if let Some(error) = pollster::block_on(scope.pop()) {
                return Err(format!("shader pipeline: {error}"));
            }
            if self.pipelines.len() >= 32 {
                self.pipelines.clear();
            }
            self.pipelines.insert(key.clone(), pipeline);
        }
        self.pipelines.get(&key).ok_or_else(|| "shader pipeline cache failed".into())
    }

    fn render(&mut self, def: &ShaderDefinition, values: &BTreeMap<String, f32>, width: u32, height: u32, time: f32) -> Result<Vec<u8>, String> {
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err("shader texture dimensions exceed Phase 1 limit".into());
        }
        let pipeline = self.pipeline(def)?.clone();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard shader fill target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut uniforms = [0u8; 16 + MAX_PARAMETERS * 16];
        uniforms[0..4].copy_from_slice(&(width as f32).to_le_bytes());
        uniforms[4..8].copy_from_slice(&(height as f32).to_le_bytes());
        uniforms[8..12].copy_from_slice(&if def.features.animated && time.is_finite() { time.max(0.0) } else { 0.0 }.to_le_bytes());
        for param in &def.parameters {
            let offset = 16 + usize::from(param.slot) * 16;
            if param.control == ShaderParameterControl::Color {
                let color = param.color_value(values);
                for (channel, value) in [color.r, color.g, color.b, color.a].into_iter().enumerate() {
                    let at = offset + channel * 4;
                    uniforms[at..at + 4].copy_from_slice(&value.to_le_bytes());
                }
            } else {
                uniforms[offset..offset + 4].copy_from_slice(&param.resolved_scalar(values).to_le_bytes());
            }
        }
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard shader fill parameters"),
            size: uniforms.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform, 0, &uniforms);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard shader fill bindings"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let bytes_per_row = width.saturating_mul(4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard shader fill readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard shader fill pass") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Lizard shader fill"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &output,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        output.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(2)) })
            .map_err(|e| format!("shader GPU timeout: {e}"))?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|e| format!("shader GPU readback timeout: {e}"))?
            .map_err(|e| format!("shader GPU readback failed: {e}"))?;
        let mapped = output.get_mapped_range(..).map_err(|e| format!("shader GPU buffer mapping failed: {e}"))?;
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            rgba.extend_from_slice(&mapped[start..start + width as usize * 4]);
        }
        drop(mapped);
        output.unmap();
        Ok(rgba)
    }
}

pub(super) fn render_rgba(def: &ShaderDefinition, values: &BTreeMap<String, f32>, width: u32, height: u32, time: f32) -> Result<Vec<u8>, String> {
    static RUNTIME: OnceLock<Result<Mutex<Gpu>, String>> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| Gpu::new().map(Mutex::new)).as_ref().map_err(Clone::clone)?;
    let mut gpu = runtime.lock().map_err(|_| "shader GPU lock poisoned".to_string())?;
    gpu.render(def, values, width, height, time)
}
