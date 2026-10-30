# GPU renderer migration checklist

The target is GPU rasterization for every user-facing drawing path when a compatible GPU is
available. CPU rendering remains a fallback for unavailable devices and an oracle for parity
tests. SVG and PDF keep their native vector output; only content they must rasterize belongs in
this migration. A box is complete only after native and WASM checks, representative visual parity,
and an interaction or export test. Do not remove a capability guard merely to mark a box done.

## Native editor canvas

- [x] Use Vello/wgpu as the default canvas backend for supported scenes.
- [ ] Replace the unsupported-scene CPU fallback with GPU implementations for every visible
  paint, effect, mask, blend mode, text run, and color profile. Keep a fallback only for GPU failure.
- [ ] Render outline mode, soft proof, selection/drag previews, and other canvas overlays on GPU.
- [ ] Keep pan, zoom, frame drag, scale, rotate, clone, and creation GPU-resident at interaction
  speeds for documents with mixed effects; check error paths and resize behavior.
- [ ] Remove the CPU atlas path after direct GPU covers its use cases.

## Paints, effects, and composition

- [ ] Support image filters, diamond gradients, pattern fills, shader fills, per-paint blend modes,
  non-normal node blends, complex text style runs, and Display P3 without CPU rasterization.
- [ ] Support alpha and luminance masks, nested masks, clipped groups, and their effect stacks.
- [ ] Support arbitrary ordered combinations of layer/background blur, noise, texture, shadows,
  and glass on rotated/skewed shapes and groups, including multiple shadows.
- [ ] Remove shape, spread, stroke, size, zoom, and region-of-interest limits in GPU effect passes
  without exceeding device texture limits; tile large surfaces instead.
- [ ] Measure parity and latency on the icon, a complex multi-frame document, UltraWide/Retina,
  transparent content, and color-profile fixtures.

## Raster exports and secondary views

- [x] Use GPU by default for compatible native PNG/JPEG/WebP and clipboard raster exports.
- [ ] Export all page and layer selections, including masks, backdrop effects, P3, and surfaces
  larger than 4096 px, through the GPU with correct alpha, clipping, and effect order.
- [ ] Move document thumbnail, asset/style/font previews, home cards, vectorization source,
  presentation textures, eyedropper sampling, and headless MCP renders to GPU.
- [ ] Use the same GPU effect path when SVG/PDF must embed rasterized effects or masks; keep
  their ordinary vector drawing as vector output.

## Platforms and retirement

- [ ] Enable the GPU canvas and GPU raster exports in the browser/WASM build, with WebGPU and
  a documented fallback for browsers without it.
- [ ] Remove production tiny-skia raster paths once GPU coverage is demonstrated; retain a
  test-only CPU reference and an explicit no-GPU fallback if required for accessibility.
- [ ] Pass `cargo xtask ci`, run native visual checks, compare export pixels, and benchmark pan,
  zoom, edit, and export before treating this checklist as complete.

## Evidence log

- 2026-10-30: default native GPU canvas and guarded native raster exports landed in
  `48a7977e`; `cargo xtask ci` passed. The saved icon's 1024 px GPU export differed from the CPU
  reference by about 0.8/255 per RGB channel on average. The default GPU canvas was inspected
  in a native screenshot. This does not establish coverage for the open boxes above.
- 2026-10-30: opaque page raster exports now use the editor's ordered GPU backdrop pass and
  supported alpha/vector masks. A fixture combining a mask and background blur passed sampled
  pixel comparisons against the CPU reference. Selected-layer exports still need their own
  backdrop semantics, and transparent pages need a transparent GPU backdrop target.
- 2026-10-30: the ordered page pass now accepts a transparent clear color; the mask/backdrop
  fixture passed on opaque and transparent pages. A selected masked group also passed export
  pixel samples. The engine now shares native GPU-first page and area rendering with saved-file
  thumbnails, CLI, headless MCP, asset/style/font previews, and home cards. All 711 UI library
  tests passed; presentation animation, vectorization, and rasterized SVG/PDF content remain open.
- 2026-10-30: simple leaf node and individual paint blend modes now use Vello blend layers.
  Sampled GPU/CPU pixels passed for every supported mode; Plus Darker remains guarded because
  Peniko has no corresponding operator. Seven GPU export tests passed. Complex groups and
  effect stacks with blends still need coverage.
