//! Render document shader fills into Vello-owned textures without GPU readback.

use std::collections::{BTreeMap, HashMap};

use lizard_doc::{ShaderDefinition, ShaderParameterControl};
use wgpu::util::DeviceExt;

#[derive(Default)]
pub struct GpuShader {
    pipelines: HashMap<String, wgpu::RenderPipeline>,
}

impl GpuShader {
    pub fn new() -> Self {
        Self::default()
    }

    fn pipeline(&mut self, device: &wgpu::Device, def: &ShaderDefinition) -> Result<wgpu::RenderPipeline, String> {
        let key = format!("{}\n{}", def.entry_point, def.source);
        if let Some(pipeline) = self.pipelines.get(&key) {
            return Ok(pipeline.clone());
        }
        lizard_shader::validate(def)?;
        let source = format!("{}\n{}", lizard_shader::PRELUDE, def.source);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard canvas shader fill"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard canvas shader uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lizard canvas shader pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Lizard canvas shader pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("shader_vertex"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(&def.entry_point),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    // The shader contract returns straight alpha. Clear to transparent and blend
                    // once so Vello receives the premultiplied texture it expects.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        if self.pipelines.len() >= 32 {
            self.pipelines.clear();
        }
        self.pipelines.insert(key, pipeline.clone());
        Ok(pipeline)
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        def: &ShaderDefinition,
        values: &BTreeMap<String, f32>,
        width: u32,
        height: u32,
        time: f32,
    ) -> Result<wgpu::Texture, String> {
        if width == 0
            || height == 0
            || width > lizard_shader::MAX_DIMENSION
            || height > lizard_shader::MAX_DIMENSION
            || u64::from(width) * u64::from(height) > lizard_shader::MAX_PIXELS
        {
            return Err("shader texture dimensions exceed the GPU limit".into());
        }
        let pipeline = self.pipeline(device, def)?;
        let mut uniforms = [0u8; 16 + lizard_shader::MAX_PARAMETERS * 16];
        uniforms[0..4].copy_from_slice(&(width as f32).to_le_bytes());
        uniforms[4..8].copy_from_slice(&(height as f32).to_le_bytes());
        let frame_time = if def.features.animated && time.is_finite() { time.max(0.0) } else { 0.0 };
        uniforms[8..12].copy_from_slice(&frame_time.to_le_bytes());
        for param in &def.parameters {
            let offset = 16 + usize::from(param.slot) * 16;
            if param.control == ShaderParameterControl::Color {
                let c = param.color_value(values);
                for (index, value) in [c.r, c.g, c.b, c.a].into_iter().enumerate() {
                    let at = offset + index * 4;
                    uniforms[at..at + 4].copy_from_slice(&value.to_le_bytes());
                }
            } else {
                uniforms[offset..offset + 4].copy_from_slice(&param.resolved_scalar(values).to_le_bytes());
            }
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard canvas shader values"),
            contents: &uniforms,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard canvas shader bindings"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard canvas shader texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard canvas shader encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Lizard canvas shader fill"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        Ok(texture)
    }
}
