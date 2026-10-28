//! Full viewport GPU vectors with a cached low-resolution effect difference.

use std::error::Error;
use std::sync::mpsc;

use figcraft_render::{Pixmap, View};

const SHADER: &str = r#"
@group(0) @binding(0) var sharp: texture_2d<f32>;
@group(0) @binding(1) var low_full: texture_2d<f32>;
@group(0) @binding(2) var low_base: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
struct Mapping { camera: vec4<f32>, low: vec4<f32>, size: vec4<f32> };
@group(0) @binding(4) var<uniform> mapping: Mapping;
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
    let base = textureSample(sharp, linear_sampler, input.uv);
    let world = mapping.camera.xy + input.position.xy / mapping.camera.z;
    let low_uv = (world - mapping.low.xy) * mapping.low.z / mapping.size.zw;
    if (any(low_uv < vec2<f32>(0.0)) || any(low_uv >= vec2<f32>(1.0))) {
        return base;
    }
    let effect = textureSample(low_full, linear_sampler, low_uv) - textureSample(low_base, linear_sampler, low_uv);
    return clamp(base + effect, vec4<f32>(0.0), vec4<f32>(1.0));
}
"#;

pub struct GpuLiveResidual {
    device: wgpu::Device,
    queue: wgpu::Queue,
    low_full: wgpu::Texture,
    low_base: wgpu::Texture,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    surface_pipeline: Option<wgpu::RenderPipeline>,
    bind: wgpu::BindGroup,
    alt_bind: Option<wgpu::BindGroup>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    mapping: wgpu::Buffer,
    width: u32,
    height: u32,
    low: View,
}

impl GpuLiveResidual {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        sharp: &wgpu::TextureView,
        low: View,
        width: u32,
        height: u32,
        surface_format: Option<wgpu::TextureFormat>,
    ) -> Self {
        let input_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let low_texture = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: low.width, height: low.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: input_usage,
                view_formats: &[],
            })
        };
        let low_full = low_texture("live vector full low effects");
        let low_base = low_texture("live vector low geometry");
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("live vector plus cached effects"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("live residual inputs"),
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mapping = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("live residual mapping"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("live residual bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(sharp) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&low_full.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&low_base.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: mapping.as_entire_binding() },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("live residual shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("live residual pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make_pipeline = |format, label| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(wgpu::TextureFormat::Rgba8Unorm, "live residual offscreen");
        let surface_pipeline = surface_format.map(|format| make_pipeline(format, "live residual surface"));
        Self {
            device,
            queue,
            low_full,
            low_base,
            output,
            output_view,
            pipeline,
            surface_pipeline,
            bind,
            alt_bind: None,
            layout,
            sampler,
            mapping,
            width,
            height,
            low,
        }
    }

    /// Reuse the uploaded correction while switching between live and atlas geometry.
    pub fn set_alt_sharp(&mut self, sharp: &wgpu::TextureView) {
        self.alt_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas geometry residual bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(sharp) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.low_full.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.low_base.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: self.mapping.as_entire_binding() },
            ],
        }));
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub fn low_views(&self) -> (wgpu::TextureView, wgpu::TextureView) {
        (self.low_full.create_view(&Default::default()), self.low_base.create_view(&Default::default()))
    }

    pub fn upload(&self, full: &Pixmap, base: &Pixmap) -> Result<(), Box<dyn Error>> {
        if full.width() != self.low.width || full.height() != self.low.height || base.width() != self.low.width || base.height() != self.low.height {
            return Err("low effect images have different dimensions".into());
        }
        for (texture, image) in [(&self.low_full, full), (&self.low_base, base)] {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                image.data(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.low.width * 4), rows_per_image: Some(self.low.height) },
                wgpu::Extent3d { width: self.low.width, height: self.low.height, depth_or_array_layers: 1 },
            );
        }
        Ok(())
    }

    fn render_target(&self, camera: &View, target: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline, bind: &wgpu::BindGroup) {
        let mut data = Vec::with_capacity(48);
        for value in [
            camera.origin.0 as f32,
            camera.origin.1 as f32,
            camera.scale as f32,
            0.0,
            self.low.origin.0 as f32,
            self.low.origin.1 as f32,
            self.low.scale as f32,
            0.0,
            self.width as f32,
            self.height as f32,
            self.low.width as f32,
            self.low.height as f32,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        self.queue.write_buffer(&self.mapping, 0, &data);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("live residual pass") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("live residual pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
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
    }

    pub fn render_submit(&self, camera: &View) {
        self.render_target(camera, &self.output_view, &self.pipeline, &self.bind);
    }

    pub fn render_submit_alt(&self, camera: &View) -> Result<(), Box<dyn Error>> {
        let bind = self.alt_bind.as_ref().ok_or("alternate sharp geometry not configured")?;
        self.render_target(camera, &self.output_view, &self.pipeline, bind);
        Ok(())
    }

    pub fn render_submit_to_surface(&self, camera: &View, target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        let pipeline = self.surface_pipeline.as_ref().ok_or("surface pipeline unavailable")?;
        self.render_target(camera, target, pipeline, &self.bind);
        Ok(())
    }

    pub fn render_submit_alt_to_surface(&self, camera: &View, target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        let pipeline = self.surface_pipeline.as_ref().ok_or("surface pipeline unavailable")?;
        let bind = self.alt_bind.as_ref().ok_or("alternate sharp geometry not configured")?;
        self.render_target(camera, target, pipeline, bind);
        Ok(())
    }

    pub fn wait(&self) -> Result<(), Box<dyn Error>> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    pub fn pixels(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let stride = (self.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("live residual readback"),
            size: u64::from(stride) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("live residual copy") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.wait()?;
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let mut result = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for row in mapped.chunks_exact(stride as usize) {
            result.extend_from_slice(&row[..self.width as usize * 4]);
        }
        Ok(result)
    }
}
