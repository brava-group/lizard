# Shader fills

Lizard stores WGSL shader definitions in `Document.shaders`, keyed by stable ID. A layer's ordered
`fills` list can contain `{"type":"shader","shaderId":"…","values":{"frequency":2}}`.
The definition is embedded in the `.figcraft` document; instances share its source and pipeline
but keep individual values. Existing documents load with an empty shader registry. The commands
below use the normal engine history, so applying a fill and changing a value support undo/redo.

## Editor use

Select a layer with a fill and click its paint control. In the colour picker, click the **Shader**
icon (the wavy square). A second floating panel opens to the left with live thumbnails. Click a
thumbnail to replace that paint with a shader fill. The built-in presets are **Linear Wave
Gradient**, **Moiré**, **Aurora Mesh**, **Cloud Noise**, **Voronoi**, **Checker Flow**, and
**Aurora**. Aurora is a WGSL adaptation of the supplied `aurora.glsl`. Its Speed, Flow direction,
Turbulence, Detail, four colours, palette direction, and grain switch are editable independently
on each layer.

After applying a preset, the fill row shows its name. Clicking that row reopens the picker on the
shader tab. Each parameter uses the control declared by its definition: slider, unrestricted
number field, select, switch, or colour control. The colour swatch opens a floating colour editor
to the left of the shader panel. Changes appear on the canvas while dragging and are recorded as
one undo step. The shader panel's name control reopens the preset
browser. Its **+** button creates a document-owned copy of the wave template and opens the WGSL
editor. The **•••** button edits the current shader's WGSL, which is shared by all instances with
that shader ID. The editor validates after a short pause and retains the last valid canvas result
until **Apply WGSL** is clicked. **Restore saved** discards the draft.

## Command API

```json
{"command":"shader.demo","params":{"ids":[2]}}
{"command":"shader.setParameter","params":{"ids":[2],"index":1,"parameter":"frequency","value":7.5}}
{"command":"shader.apply","params":{"ids":[2],"shaderId":"lizard.linear-wave-gradient"}}
{"command":"shader.register","params":{"definition":{"schemaVersion":1,"id":"my-fill","name":"My fill","kind":"fill","version":"1.0.0","source":"@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f { return vec4f(uv, 0.5, 1.0); }","entryPoint":"fs_main","parameters":[]}}}
```

`shader.register` validates WGSL and the initial contract before changing the document. The
definition may also specify `description`, `author`, `license`, `attribution`, `thumbnail` and
`features`. `kind` currently accepts `fill`; `effect` is reserved. Set `features.animated` to
`true` for time-based fills; `features.mouseInteraction` is not supported yet. Every parameter has
`id`, `name`, `slot` (0–15), and optional `description`. The `control` field defaults to `slider`,
so existing documents remain valid. Controls use these fields:

| `control` | Definition fields | WGSL value |
| --- | --- | --- |
| `slider` | `default`, `min`, `max`, `step` | `globals.params[slot].x` |
| `number` | `default`, optional `step`; no range clamp | `.x` |
| `select` | `default`, `options: [{label, value}]` | `.x` (selected numeric value) |
| `toggle` | `default: 0` or `1` | `.x` |
| `color` | `defaultColor: "#RRGGBB"` or RGBA | `.rgba` |

For example, a colour and select can be declared as:

```json
[
  {"id":"primary","name":"Primary","slot":0,"control":"color","defaultColor":"#0D2B3E"},
  {"id":"direction","name":"Direction","slot":1,"control":"select","default":0,
   "options":[{"label":"Forward","value":0},{"label":"Reverse","value":1}]}
]
```

Each scalar instance value stays under its `id`. A colour stores four channel keys such as
`primary.r`, `primary.g`, `primary.b`, `primary.a` in the same instance value map. The shader
definition is still stored once per document. Changing any parameter updates a uniform without recompiling the
pipeline. `globals.resolution` is the offscreen texture size in pixels; `globals.time` is the
elapsed canvas time in seconds for animated fills. Static fills and default exports use zero.
Fragment input `uv` ranges from (0,0) at the top left to (1,1) at the bottom right.

The editor requests animation frames at up to 15 fps while an animated fill is visible in the
canvas. Hidden and offscreen fills do not schedule frames. Reduced motion freezes the frame at
time zero. `render_rgba_at` and `render_node_at` accept explicit time for reproducible renders;
`render_rgba`, `render_node`, and still exports use time zero. A speed value of zero pauses Aurora.
The picker thumbnails show a still frame. Imported GLSL cannot run directly: adapt it to the
document's WGSL and uniform contract before registering it.

The runtime injects this WGSL contract ahead of the shader's source:

```wgsl
struct ShaderGlobals {
    resolution: vec2f,
    time: f32,
    padding: f32,
    params: array<vec4f, 16>,
};
@group(0) @binding(0) var<uniform> globals: ShaderGlobals;
```

Only the system uniform binding is available. The source must declare
`@fragment fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f`; Lizard supplies the
full-screen vertex stage. Source is limited to 64 KiB; render targets to 2048 pixels on each
axis and about four million pixels total. The native runtime uses `naga` for parsing and
validation, then `wgpu` for the GPU pass. It caches up to 32 pipelines by WGSL source and up to
eight compositing pixmaps. GPU validation and readback errors leave the document intact.

## Rendering boundaries

The current scene compositor uses CPU `tiny-skia`. The shader pass therefore renders offscreen on
the GPU, reads RGBA pixels back, and lets `tiny-skia` apply the layer path and paint composition.
This preserves the existing vector geometry and non-destructive document. GPU-to-CPU readback is
the main current performance cost. Large fills are capped in resolution; their edges can soften
at high zoom. On the web build, shader definitions load but this native offscreen pass does not
run yet, and thumbnails show placeholders. Shader effects, mouse interaction,
external resources, and external/Figma import remain future work. `.fig` export reports that shader fills were omitted; SVG export embeds
the rendered fill as a PNG pattern. PNG export uses the canvas renderer.

Future work: move shader composition onto the existing egui/wgpu GPU path, provide asynchronous
compilation and preview, support offscreen effect passes, and add web runtime
support. External WGSL must be adapted to the binding contract before registration. Naga syntax
validation alone cannot guarantee fast GPU execution, so untrusted source needs further execution
policy before a public import UI is enabled.

API references: [wgpu 30.0.1](https://docs.rs/wgpu/30.0.1/wgpu/) and
[naga 30.0.1](https://docs.rs/naga/30.0.1/naga/).
