//! Convert document Display P3 pixels to sRGB for native screen presentation on the GPU.

const SHADER: &str = r#"
struct Params {
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
    dimensions: vec4<u32>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn decode(encoded: vec3<f32>) -> vec3<f32> {
    let value = clamp(encoded, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(value / 12.92, pow((value + 0.055) / 1.055, vec3<f32>(2.4)), value > vec3<f32>(0.04045));
}

fn encode(linear: vec3<f32>) -> vec3<f32> {
    let value = clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(value * 12.92, 1.055 * pow(value, vec3<f32>(1.0 / 2.4)) - 0.055, value > vec3<f32>(0.0031308));
}

fn encoded_positive(value: f32) -> f32 {
    if value <= 0.0031308 { return value * 12.92; }
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

fn outside_srgb(value: f32) -> bool {
    let slack = 2.0 / 255.0;
    return (value < 0.0 && encoded_positive(-value) > slack)
        || (value > 1.0 && encoded_positive(value) > 1.0 + slack);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.dimensions.x || id.y >= params.dimensions.y { return; }
    let xy = vec2<i32>(id.xy);
    let pixel = textureLoad(source, xy, 0);
    if pixel.a <= 0.0 {
        textureStore(destination, xy, vec4<f32>(0.0));
        return;
    }
    let linear = decode(pixel.rgb / pixel.a);
    let converted = vec3<f32>(
        dot(params.row0.xyz, linear),
        dot(params.row1.xyz, linear),
        dot(params.row2.xyz, linear),
    );
    if params.dimensions.z != 0u
        && (outside_srgb(converted.r) || outside_srgb(converted.g) || outside_srgb(converted.b))
        && (id.x + id.y) % 8u < 4u {
        textureStore(destination, xy, vec4<f32>(128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0));
        return;
    }
    textureStore(destination, xy, vec4<f32>(encode(converted) * pixel.a, pixel.a));
}
"#;

pub struct GpuColorProfile {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    output: wgpu::Texture,
    view: wgpu::TextureView,
}

impl GpuColorProfile {
    pub fn new(device: &wgpu::Device, size: wgpu::Extent3d) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lizard Display P3 to sRGB"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lizard Display P3 bindings"),
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
            label: Some("Lizard Display P3 pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Lizard Display P3 pass"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Lizard sRGB presentation"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = output.create_view(&wgpu::TextureViewDescriptor::default());
        Self { pipeline, layout, output, view }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.output
    }

    pub fn submit(&self, device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, matrix: [[f32; 3]; 3], proof: bool) {
        let size = source.size();
        let mut bytes = Vec::with_capacity(64);
        for row in matrix {
            for value in [row[0], row[1], row[2], 0.0] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        for value in [size.width, size.height, u32::from(proof), 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lizard Display P3 matrix"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform, 0, &bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lizard Display P3 group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.view) },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lizard Display P3 conversion") });
        {
            let mut pass =
                encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lizard Display P3 conversion"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.width.div_ceil(16), size.height.div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
    }
}
