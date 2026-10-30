// Port of the user's aurora.glsl. Coordinates use the editor's top-left UV convention.
fn aurora_hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(12.9898, 78.233))) * 43758.5453);
}

fn aurora_octave(p: vec2f, t: f32) -> f32 {
    var v = sin(p.x * 3.0 + t);
    v += sin(p.y * 3.7 - t * 0.8);
    v += sin((p.x + p.y) * 2.3 + t * 0.6);
    v += sin(length(p - vec2f(0.5)) * 6.0 - t);
    return v * 0.25;
}

fn aurora_fbm(position: vec2f, phase: f32) -> f32 {
    var p = position;
    var t = phase;
    var v = 0.0;
    var amp = 0.65;
    for (var i = 0; i < 3; i = i + 1) {
        v += aurora_octave(p, t) * amp;
        p *= 2.0;
        t *= 1.2;
        amp *= 0.5;
    }
    return v;
}

fn aurora_flow(p: vec2f, t: f32) -> vec2f {
    let q = vec2f(aurora_fbm(p, t), aurora_fbm(p + vec2f(5.2, 1.3), t));
    let r = vec2f(
        aurora_fbm(p + 1.8 * q + vec2f(1.7, 9.2), t * 0.6),
        aurora_fbm(p + 1.8 * q + vec2f(8.3, 2.8), t * 0.6)
    );
    return vec2f(aurora_fbm(p + 2.0 * r, t), length(r));
}

fn aurora_ease(x: f32) -> f32 {
    return x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
}

fn aurora_palette(t: f32) -> vec4f {
    let c1 = globals.params[4];
    let c2 = globals.params[5];
    let c3 = globals.params[6];
    let c4 = globals.params[7];
    let segment = fract(t) * 4.0;
    let fade = aurora_ease(fract(segment));
    if (segment < 1.0) { return mix(c1, c2, fade); }
    if (segment < 2.0) { return mix(c2, c3, fade); }
    if (segment < 3.0) { return mix(c3, c4, fade); }
    return mix(c4, c1, fade);
}

@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let speed = globals.params[0].x;
    let angle_degrees = globals.params[1].x;
    let turbulence = globals.params[2].x;
    let detail = globals.params[3].x;
    let angle = radians(angle_degrees);
    let direction = vec2f(cos(angle), sin(angle));
    let gradient = dot(uv - vec2f(0.5), direction) + 0.5;
    let t = globals.time * speed;
    let drift = direction * (0.5 * t);
    let flow = aurora_flow(uv * detail + drift + vec2f(angle_degrees), t);
    let warp = flow.y;
    let palette_direction = globals.params[8].x;
    let palette_t = gradient * 0.3 + t + flow.x * turbulence;
    let palette = aurora_palette(select(palette_t, -palette_t, palette_direction > 0.5));
    var color = palette.rgb;
    color += smoothstep(0.5, 1.4, warp) * 0.18 * vec3f(0.9, 0.95, 1.0);
    color *= 0.78 + 0.22 * (1.0 - smoothstep(0.0, 1.3, warp));
    color += (aurora_hash(uv * globals.resolution) - 0.5) * 0.04 * globals.params[9].x;
    return vec4f(clamp(color, vec3f(0.0), vec3f(1.0)), palette.a);
}
