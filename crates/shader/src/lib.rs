//! Versioned shader fill contract and a bounded, offscreen WebGPU runtime.
//! The scene compositor keeps ownership of geometry, clipping and blend modes.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lizard_color::Color;
use lizard_doc::{ShaderDefinition, ShaderFeatures, ShaderFloatParameter, ShaderKind, ShaderParameterControl, ShaderSelectOption};

#[cfg(not(target_arch = "wasm32"))]
mod runtime;

pub const DEMO_ID: &str = "lizard.linear-wave-gradient";
pub const MOIRE_ID: &str = "lizard.moire";
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_DIMENSION: u32 = 2048;
pub const MAX_PIXELS: u64 = 4_194_304;
pub const MAX_PARAMETERS: usize = 16;

/// The fixed Phase 1 binding contract. Custom WGSL defines only `fs_main` and helper functions.
pub const PRELUDE: &str = r#"
struct ShaderGlobals {
    resolution: vec2f,
    time: f32,
    padding: f32,
    params: array<vec4f, 16>,
};
@group(0) @binding(0) var<uniform> globals: ShaderGlobals;
struct ShaderVertex {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};
@vertex fn shader_vertex(@builtin(vertex_index) index: u32) -> ShaderVertex {
    var positions = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    var out: ShaderVertex;
    out.position = vec4f(positions[index], 0.0, 1.0);
    out.uv = vec2f((out.position.x + 1.0) * 0.5, (1.0 - out.position.y) * 0.5);
    return out;
}
"#;

/// Validates the exact WGSL and binding contract before a pipeline is created.
pub fn validate(def: &ShaderDefinition) -> Result<(), String> {
    if def.schema_version != 1 {
        return Err("unsupported shader schema version".into());
    }
    if def.kind != ShaderKind::Fill {
        return Err("shader effects are not supported in Phase 1".into());
    }
    if def.features.mouse_interaction {
        return Err("mouse-interactive shaders are not supported yet".into());
    }
    if def.id.is_empty() || def.id.len() > 128 {
        return Err("shader id must have 1–128 bytes".into());
    }
    if def.source.len() > MAX_SOURCE_BYTES {
        return Err("WGSL source exceeds 64 KiB".into());
    }
    if def.entry_point != "fs_main" {
        return Err("Phase 1 requires entry point fs_main".into());
    }
    if def.parameters.len() > MAX_PARAMETERS {
        return Err("at most 16 parameter slots are supported".into());
    }
    let mut slots = [false; MAX_PARAMETERS];
    let mut names = std::collections::BTreeSet::new();
    for p in &def.parameters {
        let slot = usize::from(p.slot);
        if slot >= MAX_PARAMETERS || slots[slot] || p.id.is_empty() || !names.insert(p.id.as_str()) {
            return Err(format!("duplicate or invalid parameter: {}", p.id));
        }
        if ![p.default, p.min, p.max, p.step].into_iter().all(f32::is_finite) || p.step <= 0.0 {
            return Err(format!("invalid range for parameter: {}", p.id));
        }
        match p.control {
            ShaderParameterControl::Slider if p.min >= p.max || p.default < p.min || p.default > p.max => {
                return Err(format!("invalid slider range for parameter: {}", p.id));
            }
            ShaderParameterControl::Select => {
                if p.options.is_empty()
                    || p.options.len() > 64
                    || p.options.iter().any(|option| option.label.is_empty() || option.label.len() > 128 || !option.value.is_finite())
                    || !p.options.iter().any(|option| option.value == p.default)
                {
                    return Err(format!("invalid select options for parameter: {}", p.id));
                }
                let unique: std::collections::BTreeSet<_> = p.options.iter().map(|option| option.value.to_bits()).collect();
                if unique.len() != p.options.len() {
                    return Err(format!("duplicate select value for parameter: {}", p.id));
                }
            }
            ShaderParameterControl::Toggle if p.default != 0.0 && p.default != 1.0 => {
                return Err(format!("toggle default must be 0 or 1: {}", p.id));
            }
            ShaderParameterControl::Color if p.default_color.is_none() => {
                return Err(format!("color parameter needs defaultColor: {}", p.id));
            }
            _ => {}
        }
        if p.control != ShaderParameterControl::Select && !p.options.is_empty() {
            return Err(format!("options require a select control: {}", p.id));
        }
        slots[slot] = true;
    }
    if def.source.contains("@group") || def.source.contains("@binding") {
        return Err("external bindings are unavailable in Phase 1".into());
    }
    let source = format!("{PRELUDE}\n{}", def.source);
    let module = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.emit_to_string(&source))?;
    if !module.entry_points.iter().any(|e| e.name == def.entry_point && e.stage == naga::ShaderStage::Fragment) {
        return Err("fragment entry point fs_main is missing".into());
    }
    if module.entry_points.len() != 2 || module.global_variables.len() != 1 {
        return Err("only one fragment entry point and the system uniform binding are allowed".into());
    }
    Ok(())
}

/// Bundled shader stored in the document by the apply command, exactly like a user definition.
pub fn demo() -> ShaderDefinition {
    ShaderDefinition {
        schema_version: 1,
        id: DEMO_ID.into(),
        name: "Linear Wave Gradient".into(),
        kind: ShaderKind::Fill,
        description: "A procedural wave between violet and cyan".into(),
        author: "Lizard".into(),
        version: "1.0.0".into(),
        license: "MIT OR Apache-2.0".into(),
        attribution: "Lizard".into(),
        features: ShaderFeatures::default(),
        thumbnail: None,
        entry_point: "fs_main".into(),
        source: r#"
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let frequency = globals.params[0].x;
    let wave = sin(uv.x * frequency * 6.2831853 + uv.y * 5.0) * 0.12;
    let t = clamp(uv.x + wave, 0.0, 1.0);
    return vec4f(mix(vec3f(0.40, 0.22, 1.0), vec3f(0.0, 0.86, 0.92), t), 1.0);
}
"#
        .into(),
        parameters: vec![ShaderFloatParameter {
            id: "frequency".into(),
            name: "Frequency".into(),
            slot: 0,
            default: 2.0,
            min: 0.0,
            max: 12.0,
            step: 0.1,
            description: "Number of horizontal waves".into(),
            control: ShaderParameterControl::Slider,
            options: vec![],
            default_color: None,
        }],
    }
}

/// First-party presets. All use the same contract and validation as document-owned WGSL.
pub fn presets() -> Vec<ShaderDefinition> {
    fn param(id: &str, name: &str, slot: u8, default: f32, min: f32, max: f32, step: f32) -> ShaderFloatParameter {
        ShaderFloatParameter {
            id: id.into(),
            name: name.into(),
            slot,
            default,
            min,
            max,
            step,
            description: String::new(),
            control: ShaderParameterControl::Slider,
            options: vec![],
            default_color: None,
        }
    }
    fn color_param(id: &str, name: &str, slot: u8, hex: &str) -> ShaderFloatParameter {
        ShaderFloatParameter {
            control: ShaderParameterControl::Color,
            default_color: Color::from_hex(hex),
            ..param(id, name, slot, 0.0, 0.0, 1.0, 0.01)
        }
    }
    fn preset(id: &str, name: &str, description: &str, source: &str, parameters: Vec<ShaderFloatParameter>) -> ShaderDefinition {
        ShaderDefinition {
            schema_version: 1,
            id: id.into(),
            name: name.into(),
            kind: ShaderKind::Fill,
            description: description.into(),
            author: "Lizard".into(),
            version: "1.0.0".into(),
            license: "MIT OR Apache-2.0".into(),
            attribution: "Lizard".into(),
            features: ShaderFeatures::default(),
            thumbnail: None,
            source: source.into(),
            entry_point: "fs_main".into(),
            parameters,
        }
    }
    let mut items = vec![
        demo(),
        preset(
            MOIRE_ID,
            "Moiré",
            "Interfering curved lines with optional RGB separation",
            r#"
fn lines(uv: vec2f, count: f32, offset: f32) -> f32 {
    let a = length(uv - vec2f(0.15, 0.48));
    let b = length(uv - vec2f(0.85, 0.52));
    let phase = (a - b) * count * 6.2831853 + offset;
    return 1.0 - smoothstep(0.75, 0.94, abs(sin(phase)));
}
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let count = globals.params[0].x;
    let scale = globals.params[1].x;
    let split = globals.params[2].x * 0.01;
    let offset = globals.params[3].x;
    let p = (uv - vec2f(0.5)) * scale + vec2f(0.5);
    let r = lines(p + vec2f(split, 0.0), count, offset);
    let g = lines(p, count, offset);
    let b = lines(p - vec2f(split, 0.0), count, offset);
    return vec4f(vec3f(r, g, b) * 0.9 + vec3f(0.06, 0.04, 0.12), 1.0);
}
"#,
            vec![
                param("lineCount", "Line count", 0, 40.0, 4.0, 100.0, 1.0),
                param("scale", "Scale", 1, 1.0, 0.2, 4.0, 0.05),
                param("rgbSplit", "RGB split", 2, 0.0, 0.0, 10.0, 0.1),
                ShaderFloatParameter {
                    control: ShaderParameterControl::Number,
                    ..param("lineOffset", "Line offset", 3, 0.0, -std::f32::consts::TAU, std::f32::consts::TAU, 0.05)
                },
            ],
        ),
        preset(
            "lizard.aurora-mesh",
            "Aurora Mesh",
            "Soft overlapping procedural colour bands",
            r#"
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let bend = globals.params[0].x;
    let density = globals.params[1].x;
    let hue = globals.params[2].x;
    let a = sin(uv.x * density * 6.2831853 + uv.y * 4.0) * bend;
    let b = sin(uv.y * 5.3 - uv.x * 3.1) * 0.20;
    let t = clamp(uv.y + a + b, 0.0, 1.0);
    let left = mix(vec3f(0.15, 0.06, 0.42), vec3f(0.25, 0.72, 0.96), t);
    let right = mix(vec3f(0.92, 0.20, 0.58), vec3f(0.89, 0.98, 0.62), t);
    return vec4f(mix(left, right, clamp(uv.x + hue, 0.0, 1.0)), 1.0);
}
"#,
            vec![
                param("bend", "Bend", 0, 0.24, 0.0, 0.8, 0.01),
                param("density", "Density", 1, 1.5, 0.2, 6.0, 0.1),
                param("hue", "Hue shift", 2, 0.0, -1.0, 1.0, 0.01),
            ],
        ),
        preset(
            "lizard.cloud-noise",
            "Cloud Noise",
            "Layered value noise",
            r#"
fn random(p: vec2f) -> f32 { return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453); }
fn noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(random(i), random(i + vec2f(1.0, 0.0)), u.x),
               mix(random(i + vec2f(0.0, 1.0)), random(i + vec2f(1.0, 1.0)), u.x), u.y);
}
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let scale = globals.params[0].x;
    let contrast = globals.params[1].x;
    let seed = globals.params[2].x;
    let p = uv * scale + vec2f(seed, seed * 1.7);
    let n = noise(p) * 0.60 + noise(p * 2.0) * 0.30 + noise(p * 4.0) * 0.10;
    let v = clamp((n - 0.5) * contrast + 0.5, 0.0, 1.0);
    return vec4f(mix(vec3f(0.08, 0.10, 0.26), vec3f(0.88, 0.90, 1.0), v), 1.0);
}
"#,
            vec![
                param("scale", "Scale", 0, 5.0, 1.0, 20.0, 0.1),
                param("contrast", "Contrast", 1, 2.0, 0.2, 5.0, 0.1),
                param("seed", "Seed", 2, 1.0, 0.0, 100.0, 1.0),
            ],
        ),
        preset(
            "lizard.voronoi",
            "Voronoi",
            "Cellular shapes with bright boundaries",
            r#"
fn random2(p: vec2f) -> vec2f {
    return fract(sin(vec2f(dot(p, vec2f(127.1, 311.7)), dot(p, vec2f(269.5, 183.3)))) * 43758.5453);
}
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let cells = globals.params[0].x;
    let edge = globals.params[1].x;
    let p = uv * cells;
    let cell = floor(p);
    let f = fract(p);
    var nearest = 4.0;
    var second = 4.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let offset = vec2f(f32(x), f32(y));
            let point = offset + 0.25 + random2(cell + offset) * 0.5;
            let distance = length(point - f);
            if (distance < nearest) { second = nearest; nearest = distance; }
            else if (distance < second) { second = distance; }
        }
    }
    let border = 1.0 - smoothstep(edge, edge + 0.06, second - nearest);
    return vec4f(mix(vec3f(0.09, 0.19, 0.32), vec3f(0.22, 0.88, 0.82), nearest) + border * vec3f(0.54, 0.40, 0.38), 1.0);
}
"#,
            vec![param("cells", "Cells", 0, 7.0, 2.0, 20.0, 1.0), param("edge", "Edge", 1, 0.10, 0.01, 0.5, 0.01)],
        ),
        preset(
            "lizard.checker-flow",
            "Checker Flow",
            "Warped checkerboard pattern",
            r#"
@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let tiles = globals.params[0].x;
    let warp = globals.params[1].x;
    let twist = globals.params[2].x;
    let p = uv * tiles;
    let q = p + vec2f(sin(p.y * twist), cos(p.x * twist)) * warp;
    let a = (i32(floor(q.x)) + i32(floor(q.y))) & 1;
    let light = select(0.0, 1.0, a == 1);
    return vec4f(mix(vec3f(0.12, 0.08, 0.26), vec3f(0.96, 0.76, 0.34), light), 1.0);
}
"#,
            vec![
                param("tiles", "Tiles", 0, 8.0, 2.0, 32.0, 1.0),
                param("warp", "Warp", 1, 0.18, 0.0, 1.0, 0.01),
                param("twist", "Twist", 2, 1.0, 0.1, 8.0, 0.1),
            ],
        ),
    ];
    let mut aurora = preset(
        "lizard.aurora",
        "Aurora",
        "Animated flowing aurora, adapted from aurora.glsl",
        include_str!("aurora.wgsl"),
        vec![
            param("speed", "Speed", 0, 0.2, 0.0, 2.0, 0.01),
            param("gradientAngle", "Flow direction", 1, 45.0, 0.0, 360.0, 1.0),
            param("turbulence", "Turbulence", 2, 0.6, 0.0, 2.0, 0.01),
            param("detail", "Detail", 3, 0.55, 0.0, 2.0, 0.01),
            color_param("color1", "Color 1", 4, "0D2B3E"),
            color_param("color2", "Color 2", 5, "1B6B5A"),
            color_param("color3", "Color 3", 6, "46C6C0"),
            color_param("color4", "Color 4", 7, "9B7FD4"),
            ShaderFloatParameter {
                control: ShaderParameterControl::Select,
                options: vec![ShaderSelectOption { label: "Forward".into(), value: 0.0 }, ShaderSelectOption { label: "Reverse".into(), value: 1.0 }],
                ..param("paletteDirection", "Palette", 8, 0.0, 0.0, 1.0, 1.0)
            },
            ShaderFloatParameter { control: ShaderParameterControl::Toggle, ..param("grain", "Grain", 9, 1.0, 0.0, 1.0, 1.0) },
        ],
    );
    aurora.features.animated = true;
    aurora.attribution = "Adapted from the user's aurora.glsl".into();
    items.push(aurora);
    items
}

/// RGBA pixels, row-major, unpremultiplied. Runtime errors never change document data.
#[cfg(not(target_arch = "wasm32"))]
pub fn render_rgba(def: &ShaderDefinition, values: &std::collections::BTreeMap<String, f32>, width: u32, height: u32) -> Result<Vec<u8>, String> {
    render_rgba_at(def, values, width, height, 0.0)
}

/// Render a deterministic frame at `time` seconds. Static callers use [`render_rgba`].
#[cfg(not(target_arch = "wasm32"))]
pub fn render_rgba_at(
    def: &ShaderDefinition,
    values: &std::collections::BTreeMap<String, f32>,
    width: u32,
    height: u32,
    time: f32,
) -> Result<Vec<u8>, String> {
    runtime::render_rgba(def, values, width, height, time)
}

#[cfg(target_arch = "wasm32")]
pub fn render_rgba(_def: &ShaderDefinition, _values: &std::collections::BTreeMap<String, f32>, _width: u32, _height: u32) -> Result<Vec<u8>, String> {
    Err("WGSL offscreen rendering is unavailable in the web build in Phase 1".into())
}

#[cfg(target_arch = "wasm32")]
pub fn render_rgba_at(
    _def: &ShaderDefinition,
    _values: &std::collections::BTreeMap<String, f32>,
    _width: u32,
    _height: u32,
    _time: f32,
) -> Result<Vec<u8>, String> {
    Err("WGSL offscreen rendering is unavailable in the web build in Phase 1".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_valid() {
        assert!(validate(&demo()).is_ok());
    }

    #[test]
    fn bundled_presets_follow_the_wgsl_contract() {
        for definition in presets() {
            assert!(validate(&definition).is_ok(), "{}", definition.id);
        }
    }

    #[test]
    fn reports_wgsl_errors() {
        let mut d = demo();
        d.source = "@fragment fn fs_main() -> @location(0) vec4f { invalid }".into();
        assert!(validate(&d).is_err());
    }

    #[test]
    fn rejects_extra_bindings() {
        let mut d = demo();
        d.source.push_str("\n@group(1) @binding(0) var t: texture_2d<f32>;");
        assert!(validate(&d).is_err());
    }

    #[test]
    fn typed_controls_validate_and_preserve_unbounded_numbers() {
        let definitions = presets();
        let aurora = definitions.iter().find(|definition| definition.id == "lizard.aurora");
        assert!(aurora.is_some());
        let Some(aurora) = aurora else { return };
        assert!(validate(aurora).is_ok());
        let color = aurora.parameters.iter().find(|parameter| parameter.id == "color1");
        assert!(color.is_some());
        let Some(color) = color else { return };
        let mut values = color.default_entries().into_iter().collect();
        assert_eq!(color.color_value(&values).to_hex(), "0D2B3E");
        color.set_color(&mut values, Color::from_hex("FF8800").unwrap_or_default());
        assert_eq!(color.color_value(&values).to_hex(), "FF8800");
        let moire = definitions.iter().find(|definition| definition.id == MOIRE_ID);
        assert!(moire.is_some());
        let Some(moire) = moire else { return };
        let offset = moire.parameters.iter().find(|parameter| parameter.id == "lineOffset");
        assert_eq!(offset.and_then(|parameter| parameter.scalar_value(1000.0)), Some(1000.0));
        let palette = aurora.parameters.iter().find(|parameter| parameter.id == "paletteDirection");
        assert_eq!(palette.and_then(|parameter| parameter.scalar_value(1.0)), Some(1.0));
        assert_eq!(palette.and_then(|parameter| parameter.scalar_value(0.5)), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn gpu_demo_renders_and_uniform_changes_pixels() {
        use std::collections::BTreeMap;
        let def = demo();
        let first = render_rgba(&def, &BTreeMap::new(), 32, 16).expect("GPU demo should render");
        assert_eq!(first.len(), 32 * 16 * 4);
        assert_ne!(&first[0..3], &first[31 * 4..31 * 4 + 3]);
        let changed = render_rgba(&def, &BTreeMap::from([("frequency".into(), 8.0)]), 32, 16).expect("changed uniform should render");
        assert_ne!(first, changed);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bundled_presets_render_distinct_thumbnails() {
        use std::collections::BTreeSet;
        let mut signatures = BTreeSet::new();
        for definition in presets() {
            let values = definition.parameters.iter().flat_map(ShaderFloatParameter::default_entries).collect();
            let rgba = render_rgba(&definition, &values, 64, 64).expect("preset should render on GPU");
            assert_eq!(rgba.len(), 64 * 64 * 4, "{}", definition.id);
            assert!(rgba.chunks_exact(4).any(|pixel| pixel[0..3] != rgba[0..3]), "{} should vary across its thumbnail", definition.id);
            signatures.insert(rgba[0..16].to_vec());
        }
        assert_eq!(signatures.len(), presets().len());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn aurora_advances_with_time_and_static_shaders_do_not() {
        use std::collections::BTreeMap;
        let presets = presets();
        let Some(aurora) = presets.iter().find(|preset| preset.id == "lizard.aurora") else { panic!("Aurora preset missing") };
        assert!(aurora.features.animated);
        let values = BTreeMap::new();
        let first = render_rgba_at(aurora, &values, 64, 64, 0.0).expect("Aurora first frame");
        let second = render_rgba_at(aurora, &values, 64, 64, 1.0).expect("Aurora second frame");
        assert_ne!(first, second);
        let paused = render_rgba_at(aurora, &BTreeMap::from([("speed".into(), 0.0)]), 64, 64, 1.0).expect("Aurora paused frame");
        assert_eq!(first, paused);
        let static_fill = demo();
        assert_eq!(
            render_rgba_at(&static_fill, &values, 32, 32, 0.0).expect("static first frame"),
            render_rgba_at(&static_fill, &values, 32, 32, 1.0).expect("static second frame")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn aurora_color_and_select_controls_change_pixels_without_recompiling() {
        use std::collections::BTreeMap;
        let aurora = presets().into_iter().find(|preset| preset.id == "lizard.aurora");
        assert!(aurora.is_some());
        let Some(aurora) = aurora else { return };
        let mut values: BTreeMap<_, _> = aurora.parameters.iter().flat_map(ShaderFloatParameter::default_entries).collect();
        let baseline = render_rgba_at(&aurora, &values, 48, 48, 0.25).expect("Aurora baseline");
        let color = aurora.parameters.iter().find(|parameter| parameter.id == "color1");
        assert!(color.is_some());
        if let Some(color) = color {
            color.set_color(&mut values, Color::from_hex("FF0000").unwrap_or_default());
        }
        let recolored = render_rgba_at(&aurora, &values, 48, 48, 0.25).expect("Aurora recolored");
        assert_ne!(baseline, recolored);
        values.insert("paletteDirection".into(), 1.0);
        let reversed = render_rgba_at(&aurora, &values, 48, 48, 0.25).expect("Aurora reversed");
        assert_ne!(recolored, reversed);
    }
}
