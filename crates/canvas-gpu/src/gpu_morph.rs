//! Separable alpha dilation and erosion for GPU shadow spread.

use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Params {
    radius: u32,
    grow: u32,
    horizontal: u32,
    pad: u32,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    var alpha = select(1.0, 0.0, params.grow != 0u);
    for (var step = -i32(params.radius); step <= i32(params.radius); step += 1) {
        let sample_xy = xy + select(vec2<i32>(0, step), vec2<i32>(step, 0), params.horizontal != 0u);
        if sample_xy.x >= 0 && sample_xy.y >= 0 && sample_xy.x < i32(size.x) && sample_xy.y < i32(size.y) {
            let sample_alpha = textureLoad(source, sample_xy, 0).a;
            alpha = select(min(alpha, sample_alpha), max(alpha, sample_alpha), params.grow != 0u);
        }
    }
    textureStore(output, xy, vec4<f32>(alpha));
}
"#;

pub struct GpuMorph {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuMorph {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard GPU shadow spread"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard GPU shadow spread bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lizard GPU shadow spread pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard GPU shadow spread pass"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    fn texture(device: &wgpu::Device, size: wgpu::Extent3d, label: &'static str) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        output: &wgpu::Texture,
        radius: u32,
        grow: bool,
        horizontal: bool,
    ) {
        let mut bytes = Vec::with_capacity(16);
        for value in [radius, u32::from(grow), u32::from(horizontal), 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard GPU shadow spread parameters"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard GPU shadow spread group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&output.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU shadow spread") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard GPU shadow spread"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(source.width().div_ceil(16), source.height().div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
    }

    pub fn submit(&self, device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, radius: u32, grow: bool) -> wgpu::Texture {
        let intermediate = Self::texture(device, source.size(), "Lizard GPU horizontal shadow spread");
        let result = Self::texture(device, source.size(), "Lizard GPU vertical shadow spread");
        self.run(device, queue, source, &intermediate, radius, grow, true);
        self.run(device, queue, &intermediate, &result, radius, grow, false);
        result
    }
}
