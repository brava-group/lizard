//! Hide a cached drop shadow inside its vector silhouette without a CPU readback.

const SHADER: &str = r#"
@group(0) @binding(0) var shadow: texture_2d<f32>;
@group(0) @binding(1) var silhouette: texture_2d<f32>;
@group(0) @binding(2) var result: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(shadow);
    if id.x >= size.x || id.y >= size.y { return; }
    let xy = vec2<i32>(id.xy);
    let coverage = textureLoad(silhouette, xy, 0).a;
    textureStore(result, xy, textureLoad(shadow, xy, 0) * (1.0 - coverage));
}
"#;

pub struct GpuShadowClip {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuShadowClip {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard GPU shadow silhouette clip"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let sampled = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard GPU shadow silhouette clip bindings"),
            entries: &[
                sampled(0),
                sampled(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lizard GPU shadow silhouette clip pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard GPU shadow silhouette clip pass"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    pub fn submit(&self, device: &wgpu::Device, queue: &wgpu::Queue, shadow: &wgpu::Texture, silhouette: &wgpu::Texture) -> wgpu::Texture {
        let size = shadow.size();
        let result = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard GPU clipped shadow"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard GPU shadow silhouette clip group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&silhouette.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&result.create_view(&wgpu::TextureViewDescriptor::default())),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU shadow silhouette clip") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard GPU shadow silhouette clip"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
        result
    }
}
