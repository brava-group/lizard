//! Image adjustments run after image decoding, without a CPU pixel pass or GPU readback.

use lizard_color::ImageFilters;
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Params {
    first: vec4<f32>,
    second: vec4<f32>,
    dimensions: vec4<u32>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.dimensions.x || id.y >= params.dimensions.y { return; }
    let xy = vec2<i32>(id.xy);
    let pixel = textureLoad(source, xy, 0);
    let alpha = pixel.a;
    if alpha <= 0.0 {
        textureStore(destination, xy, pixel);
        return;
    }
    var rgb = pixel.rgb / alpha;
    rgb *= exp2(clamp(params.first.x, -1.0, 1.0) * 2.0);
    rgb = (rgb - vec3<f32>(0.5)) * (1.0 + clamp(params.first.y, -1.0, 1.0)) + vec3<f32>(0.5);
    let luminance = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = vec3<f32>(luminance) + (rgb - vec3<f32>(luminance)) * (1.0 + clamp(params.first.z, -1.0, 1.0));
    let temperature = clamp(params.first.w, -1.0, 1.0) * 0.15;
    let tint = clamp(params.second.x, -1.0, 1.0) * 0.15;
    rgb += vec3<f32>(temperature, tint, -temperature);
    let adjusted_luminance = clamp(dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let highlight = clamp(params.second.y, -1.0, 1.0) * 0.3;
    let shadow = clamp(params.second.z, -1.0, 1.0) * 0.3;
    let adjustment = highlight * adjusted_luminance * adjusted_luminance
        + shadow * (1.0 - adjusted_luminance) * (1.0 - adjusted_luminance);
    rgb += vec3<f32>(adjustment);
    textureStore(destination, xy, vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * alpha, alpha));
}
"#;

pub struct GpuImageFilter {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl GpuImageFilter {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard GPU image adjustments"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard GPU image adjustment bindings"),
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
            label: Some("Lizard GPU image adjustment pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard GPU image adjustment pass"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    pub fn submit(&self, device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, filters: &ImageFilters) -> wgpu::Texture {
        let size = source.size();
        let result = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard GPU adjusted image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let mut bytes = Vec::with_capacity(48);
        for value in
            [filters.exposure, filters.contrast, filters.saturation, filters.temperature, filters.tint, filters.highlights, filters.shadows, 0.0]
        {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [size.width, size.height, 0, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lizard GPU image adjustment params"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard GPU image adjustment group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&result.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard GPU image adjustments") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard GPU image adjustment pass"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
        result
    }
}
