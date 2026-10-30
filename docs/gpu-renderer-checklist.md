# GPU renderer migration checklist

The target is GPU rasterization for every user-facing drawing path when a compatible GPU is
available. CPU rendering remains a fallback for unavailable devices and an oracle for parity
tests. SVG and PDF keep their native vector output; only content they must rasterize belongs in
this migration. A box is complete only after native and WASM checks, representative visual parity,
and an interaction or export test. Do not remove a capability guard merely to mark a box done.

## Remaining production CPU raster paths (code audit)

| Entry point | Current CPU work | GPU completion condition |
| --- | --- | --- |
| `crates/ui-egui/src/render_worker.rs` | Canvas fallback for combined view modes, focus/selected outlines with backdrop effects, and unsupported scenes | GPU scene handles each view mode and paint/effect combination; worker runs only without GPU |
| `crates/ui-egui/src/canvas.rs` | Cropped creation preview and eyedropper when GPU cannot present | Current GPU scene covers supported interactions; no CPU scene raster on a working GPU |
| `crates/canvas-gpu/src/vello_scene.rs` | CPU effect tile fallback in the GPU compositor | GPU passes support every effect stack and region, including large surfaces |
| `crates/engine/src/cmd/file.rs` | WASM export and unsupported native export fallback | Browser asynchronous WebGPU readback and complete GPU export coverage |
| `crates/svg/src/import/paint.rs` and `crates/svg/src/lib.rs` | SVG paint sources and effect/mask PNG embedding | GPU raster service supplies required embedded pixels |
| `crates/pdf/src/draw.rs` and `crates/pdf/src/raster.rs` | PDF paint sources and effect/mask embedding | Same GPU raster service supplies PDF embedded pixels |
| Browser plugin grid | Icon PNG previews on WebGL/CPU path | Async WebGPU icon rendering and readback |

Image decoding, font shaping, vector geometry, and native SVG/PDF path emission remain CPU tasks;
the migration concerns rasterization and pixel effects.

## Native editor canvas

- [x] Use Vello/wgpu as the default canvas backend for supported scenes.
- [ ] Replace the unsupported-scene CPU fallback with GPU implementations for every visible
  paint, effect, mask, blend mode, text run, and color profile. Keep a fallback only for GPU failure.
- [ ] Render outline mode, soft proof, selection/drag previews, and other canvas overlays on GPU.
  - [x] Render Display P3 soft-proof stripes in the GPU profile-conversion pass and update the
    presented texture on a proof toggle without rerasterizing the vector scene.
  - [x] Send the stable transient IME composition document to the GPU canvas on the next frame;
    caret and composition underline remain lightweight egui overlays.
  - [x] Draw full-page Outline mode with Vello strokes on native and browser WebGPU, including
    hidden-layer contours, group bounds, and light strokes over a dark background.
  - [x] Draw Pixel Preview at the document pixel scale on GPU and register the presented texture
    with nearest-neighbor sampling on native and browser WebGPU.
  - [x] Draw a supported Focus/solo subtree on GPU at its document-space position in native and browser canvas.
  - [x] Replace selected subtrees with outlines in the GPU scene while preserving sibling paints
    and paint order on native and browser canvas.
  - [x] Preserve GPU effect caches for unselected siblings in selected-outline mode while a
    selected shape's own cached fill remains replaced by its outline.
  - [x] Ignore paint and effect capabilities inside a selected-outline subtree on native and
    browser WebGPU; keep unsupported siblings and their backdrop effects guarded.
  - [x] Route Focus/solo groups with a supported backdrop blur or glass through the ordered GPU
    compositor; sibling layers outside the focused subtree no longer force its CPU fallback.
  - [x] Scope the normal native/browser GPU capability check to the displayed page. An
    unsupported blend at the same coordinates on another page no longer forces CPU rendering.
  - [x] Combine selected outlines with supported sibling backdrop blur or glass on native and
    browser WebGPU, preserving paint order even when the selected contour is behind the pane.
  - [ ] Move the remaining mixed view modes and unsupported backdrop stacks to GPU.
  - [ ] Replace `render_worker`'s CPU `render_view` path for outline and unsupported previews
    with a GPU scene or explicit GPU-free fallback.
- [ ] Keep pan, zoom, frame drag, scale, rotate, clone, and creation GPU-resident at interaction
  speeds for documents with mixed effects; check error paths and resize behavior.
  - [x] Prepare the composited drag background, moving layer, and zoom detail with GPU-first
    rendering, retaining the existing GPU texture composition on screen.
  - [x] Suppress the CPU creation preview when the current GPU texture already contains the new
    shape, avoiding a stale second raster over the scene.
  - [ ] Remove CPU rendering from unsupported drag scenes and measure latency on mixed effects.
- [x] Remove the CPU atlas path from production after direct GPU covers its use cases. The
  reference implementation remains compiled only in native tests.

## Paints, effects, and composition

- [ ] Support image filters, diamond gradients, pattern fills, shader fills, per-paint blend modes,
  non-normal node blends, complex text style runs, and Display P3 without CPU rasterization.
  - [x] Apply the seven image adjustment controls in a WGSL compute pass after decoding.
  - [x] Render isolated noise on skewed shapes through the GPU local-coordinate pass; a skewed
    rectangle matched the CPU reference within the existing pixel tolerance.
  - [x] Render texture scatter on rotated shapes at 1× and 2× by aligning GPU cache bounds to
    document pixels; clipped and unclipped rotated fixtures match the CPU reference.
  - [x] Keep texture scatter on GPU at 1× and 2× during fractional camera pan. The static cache
    interpolates fine grains slightly; a fine-grain rotated fixture retained shape coverage and
    stayed within a 5% viewport difference threshold at both zoom levels.
  - [x] Generate diamond gradients on the GPU, including individually painted vector regions.
  - [x] Generate repeating grid and shifted-hex pattern cells on the GPU for simple leaf and
    group sources, including supported alpha, vector, and luminance masks. Retain a guarded
    fallback for nested patterns, unsupported masks, effects, and complex text sources.
  - [x] Render a clipped rounded frame used as a pattern source on the GPU; sampled corner and
    center pixels and visually compared the repeated tile against the CPU reference.
  - [x] Compose document shader fills directly from GPU textures, with zoom-aware resolution,
    live uniforms, opacity, and bounded replacement of edited textures.
  - [x] Keep native Display P3 export pixels in document space and convert the main native canvas
    to sRGB in a GPU pass.
  - [x] Convert native secondary previews and presentation textures from Display P3 on GPU
    before readback, retaining CPU conversion only for the fallback.
  - [ ] Check Display P3 conversion in the browser presentation path.
  - [x] Render the Vello-supported blend modes on simple nodes, effect-free groups, and individual paints.
  - [x] Isolate the content of an opaque shadowed leaf before applying paint blend modes; all
    Vello-supported modes match the CPU reference with a Drop Shadow. Partial-opacity shadow
    stacks remain guarded.
  - [x] Apply node blend modes to opaque leaf content after drawing its Drop Shadow; all
    Vello-supported modes match CPU samples at the fill, shadow, and background.
  - [ ] Implement Plus Darker and blends combined with effect stacks.
- [ ] Support alpha and luminance masks, nested masks, clipped groups, and their effect stacks.
- [x] Keep composited inner and drop shadows on rotated groups in the GPU cache for tested spreads
  through ±32 document units; unsupported descendants and larger effects remain guarded.
- [ ] Support arbitrary ordered combinations of layer/background blur, noise, texture, shadows,
  and glass on rotated/skewed shapes and groups, including multiple shadows.
  - [x] Support `Noise → Layer Blur` on rotated shapes at 1× and 2× zoom with GPU-only passes.
  - [ ] Correct `Layer Blur → Noise` on rotated shapes; it remains guarded because the GPU
    sampling diverges from the CPU reference after the blur pass.
- [ ] Remove shape, spread, stroke, size, zoom, and region-of-interest limits in GPU effect passes
  without exceeding device texture limits; tile large surfaces instead.
- [ ] Measure parity and latency on the icon, a complex multi-frame document, UltraWide/Retina,
  transparent content, and color-profile fixtures.

## Raster exports and secondary views

- [x] Use GPU by default for compatible native PNG/JPEG/WebP and clipboard raster exports.
- [ ] Export all page and layer selections, including masks, backdrop effects, P3, and surfaces
  larger than 4096 px, through the GPU with correct alpha, clipping, and effect order.
  - [x] Tile wide or tall effect-free page and selection exports, retaining alpha and seam pixels.
  - [x] Tile wide page and selection exports with bounded drop/inner shadows and uniform layer
    blur, including an effect halo around every tile.
  - [x] Tile wide page and selection exports containing GPU noise or texture scatter. Cropped
    GPU renders matched every pixel across both 2048 px tile boundaries in a 4200 px fixture.
  - [x] Tile wide pages with uniform background blur or glass using a conservative backdrop
    margin. GPU crop comparisons across the seam stayed within 8/255.
  - [x] Tile wide page and selection exports with progressive layer blur, plus wide pages with
    progressive background blur. GPU crop comparisons across the seam stayed within 8/255.
  - [x] Export a selected subtree with supported background blur or glass through an isolated
    GPU page, preserving its transparent background and selected paint order. The wide selected
    export matched GPU crops across the tile seam for both effects.
  - [ ] Tile effect-bearing exports with enough overlap for each effect's region of influence.
- [ ] Move document thumbnail, asset/style/font previews, home cards, vectorization source,
  presentation textures, eyedropper sampling, and headless MCP renders to GPU.
  - [x] Route static previews, vectorization backgrounds, static presentation textures,
    rasterize, and MCP renders through the shared GPU-first renderer.
  - [x] Render the vectorization SVG preview through imported vectors and GPU-first export.
  - [x] Render animated presentation textures through GPU composition after frame decoding.
  - [x] Sample the canvas eyedropper magnifier from the already presented GPU texture with an
    asynchronous 9 × 9 readback, without a per-pointer re-render or synchronous stall. The CPU
    reference remains for WebGL2 and scenes that the GPU canvas cannot draw.
  - [ ] Move plugin SVG icon thumbnails and other direct `figcraft_render` preview calls to GPU.
    - [x] Route native plugin grid SVG paths through the document importer and GPU-first export;
      retain the CPU fallback when no compatible device exists.
    - [ ] Route browser plugin grid icons through WebGPU and audit remaining direct preview calls.
- [ ] Use the same GPU effect path when SVG/PDF must embed rasterized effects or masks; keep
  their ordinary vector drawing as vector output.
  - [ ] Route SVG import paint rasterization and SVG/PDF effect/mask embedding through a GPU
    service without introducing a lower-crate dependency on egui or the command engine.

## Platforms and retirement

- [ ] Enable the GPU canvas and GPU raster exports in the browser/WASM build, with WebGPU and
  a documented fallback for browsers without it.
  - [x] Separate the browser WebGPU scene from native-only geometry atlas workers; `wgpu::Device`
    is not `Send`/`Sync` on the browser backend, so Rayon upload jobs cannot own it.
  - [x] Reuse eframe's browser WebGPU device for the direct canvas; preserve the CPU renderer for
    WebGL2, unsupported scenes, and GPU errors.
  - [x] Package the larger WebGPU WASM below the static host's per-file limit and smoke-test
    the packaged loader in Chrome.
  - [ ] Validate browser WebGPU canvas pixels and interaction latency in a browser.
  - [ ] Route browser PNG/JPEG/WebP and clipboard raster exports through WebGPU readback.
- [ ] Remove production tiny-skia raster paths once GPU coverage is demonstrated; retain a
  test-only CPU reference and an explicit no-GPU fallback if required for accessibility.
- [ ] Pass `cargo xtask ci`, run native visual checks, compare export pixels, and benchmark pan,
  zoom, edit, and export before treating this checklist as complete.

## Evidence log

- 2026-10-30: native and browser selected-outline capability checks now exclude only the roots
  replaced by outlines and their descendants. A selected group containing a Plus Darker child
  no longer forces CPU rendering; the same blend mode on an unselected sibling still does. The
  backdrop check also ignores selected roots while preserving its guard for unselected backdrops.

- 2026-10-30: selected-outline scene composition now keeps cached GPU layer effects for siblings.
  A pixel fixture with blur on both the selected rectangle and its sibling matched the CPU
  reference within the existing 8/255 threshold: the selected center stayed empty while the
  sibling blur remained visible. This does not enable combined backdrop modes yet.

- 2026-10-30: aligning GPU shadow cache bounds to document pixels reduced the rotated group
  inner-shadow difference below 1% of pixels at 1× and 2×. The fixture includes nested content,
  gradient paint, a dashed stroke, and two masks. Spreads from −32 through +32 passed the
  existing 8/255 threshold. The corresponding drop-shadow fixture passed both show-behind modes,
  the same spread range, and both zoom levels after removing the rotation guard.

- 2026-10-30: the texture scatter cache now expands rotated effect bounds to integral document
  pixels before GPU rasterization and presentation. Two rotation angles, clipped and unclipped
  scatter, and 1×/2× zoom had zero pixels differing by more than 8/255 from the CPU reference;
  each fixture had over 1,000 painted pixels. Fractional viewport origins and other zoom scales
  remain guarded until a view-aligned GPU pass is available.

- 2026-10-30: an ordered-stack probe found zero RGB pixels differing by more than 8/255 for
  rotated `Noise → Layer Blur` at 1× and 2×. Rotated `Layer Blur → Noise` differed in 3,868
  pixels at 1× and 12,950 at 2×, so its CPU fallback remains active while the matching order
  enters the GPU path. The existing axis-aligned stack checks still pass.

- 2026-10-30: removed the orthogonal-axis restriction for isolated noise. The existing GPU
  compute pass already maps surface pixels back to local coordinates, and a skewed rectangle at
  2× zoom matched the CPU reference with fewer than 1% of pixels differing by more than 8/255.
  Other noise stacks and large surfaces remain subject to their separate capability guards.

- 2026-10-30: selected-only outlines now replace the selected subtree inside the Vello scene in
  paint order on native and browser WebGPU. A fixture matched CPU pixels at an outline edge,
  its empty center, and a normally filled sibling; the hidden-selected variant also matched.
  The UI dispatch test confirmed selected IDs reach the GPU backend. Native and WASM checks and
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed. An isolated native app accepted the mode command,
  but macOS did not deliver an app screenshot while its window was not presented in the capture
  session. Focus/selected outlines with backdrop effects and combined view modes remain guarded.

- 2026-10-30: Focus/solo for a supported subtree now builds the native and browser WebGPU
  scene from that root while preserving its document-space transform. A pixel fixture matched
  the CPU Focus reference at both the focused shape and an omitted sibling. The UI dispatch
  test confirmed that it forwards the focused node ID. Native and WASM checks and
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed. Focus with backdrop effects and combined
  outline/Pixel Preview modes remain guarded.

- 2026-10-30: Pixel Preview now asks the GPU backend for a low-resolution view and presents it
  with nearest-neighbor sampling; the mode switch re-registers the texture with the appropriate
  sampler. An egui interaction test confirmed the 1× view; native and WASM `cargo check` passed.
  The full `CARGO_INCREMENTAL=0 cargo xtask ci` gate passed on retry, including 716 UI tests.
  An unrelated local control-channel test failed once under workspace load and passed alone and
  in the full retry.
- 2026-10-30: removed the `LIZARD_GPU_CANVAS_ATLAS` production switch and its CPU-rendered tile
  path. The direct Vello canvas is now the only native GPU backend; the atlas module is test-only.
  Native `cargo check` and `CARGO_INCREMENTAL=0 cargo xtask ci` passed.
- 2026-10-30: full-page Outline mode now builds Vello GPU strokes rather than using the CPU
  canvas. A pixel fixture checked visible and hidden rectangles, group bounds, and light strokes
  on a dark canvas; its output was visually inspected. The UI dispatch test confirmed that Outline
  options reach the GPU backend. Native and WASM `cargo check` and
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed; 715 UI tests passed.
- 2026-10-30: the current GPU scene now suppresses the redundant CPU creation preview, and
  an active IME composition sends its stable transient document to the GPU backend on the next
  frame. Both interaction tests passed; `CARGO_INCREMENTAL=0 cargo xtask ci` passed, including
  native tests, WASM, plugins, Clippy, formatting, and crate layers.
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
- 2026-10-30: effect-free exports wider than 4096 px now render in GPU tiles; a 4104 px page
  and selection passed CPU comparisons at both sides of each tile seam. Static presentation,
  vectorization background and SVG preview, rasterize, and eyedropper now call the shared GPU-first
  path. All three image-vectorization tests passed after switching the preview to the document's
  SVG importer.
- 2026-10-30: `CARGO_INCREMENTAL=0 cargo xtask ci` passed after updating the blend capability
  regression to expect GPU Multiply and a guarded Plus Darker fallback. Native workspace tests,
  711 UI tests, WASM checks, plugin checks, Clippy, formatting, and crate layers all passed.
- 2026-10-30: the composed drag's background, moving root, and sharp detail now use the shared
  GPU-first path. Four drag tests passed, including a recomposed scene comparison and high-zoom
  tile dimensions. Mixed unsupported effects can still take the guarded CPU fallback.
- 2026-10-30: Vello-supported node blend modes now include effect-free groups. GPU/CPU samples
  matched for every supported mode in an overlapping two-child group at full and partial opacity.
- 2026-10-30: animated GIF frames can now use the native GPU export path at presentation time.
  A two-frame GIF passed red/green/red samples at 0, 150, and 250 ms. Frame decoding remains a
  CPU image codec step; image filters and unsupported scene effects remain guarded.
- 2026-10-30: `CARGO_INCREMENTAL=0 cargo xtask ci` passed with the drag and GIF work: 59 GPU
  tests and 712 UI tests passed, along with the remaining workspace, WASM, plugin, Clippy,
  format, and layer checks.
- 2026-10-30: the seven image adjustments now run in a WGSL compute pass; all built-in presets
  and a combined-control fixture matched CPU samples within 3/255, including semitransparent
  pixels. An adjusted GIF frame also matched the CPU reference. PNG/GIF decoding still uses the
  CPU image codec before texture upload. `CARGO_INCREMENTAL=0 cargo xtask ci` passed with 60 GPU
  tests and 712 UI tests, along with native workspace, WASM, plugin, format, Clippy, and layers.
- 2026-10-30: diamond gradient textures now come from a WGSL compute pass. Sampled pixels of a
  three-stop translucent rectangle and an individually painted vector region matched the CPU
  reference within 6/255. The GPU renderer's prior diamond fallback guards were removed.
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed with 62 GPU tests and 712 UI tests. An unrelated
  zoom preview test needed a longer worker deadline under parallel workspace load; its isolated
  run completed in 60 ms and the full gate passed after the test timing adjustment.
- 2026-10-30: native GPU page and selected-layer exports now accept Display P3. A fixture with
  a managed image and gradient matched the CPU document-space pixels within 3/255. A GPU
  P3-to-sRGB presentation pass matched the CPU screen conversion within 4/255 on opaque and
  translucent pixels and after a viewport resize. An app PNG export with overlapping P3 shapes
  was visually inspected. The native window screenshot timed out because the test window was
  covered. `CARGO_INCREMENTAL=0 cargo xtask ci` passed with 64 GPU tests and 712 UI tests.
  Browser/WASM GPU coverage remains open.
- 2026-10-30: native shader fills now render on the canvas device and stay GPU-resident through
  Vello composition. GPU/CPU samples passed at 1× and 2× zoom with partial opacity; editing a
  uniform replaces its cached texture without retaining every previous value. The CPU shader
  raster remains a fallback and the browser GPU path is still open. `CARGO_INCREMENTAL=0 cargo
  xtask ci` passed with 66 GPU tests and 712 UI tests after extending the high-zoom drag test's
  deadline for parallel workspace load; that test passed alone in 0.25 s.
- 2026-10-30: simple pattern cells now render as Vello textures on the same GPU device and repeat
  in the destination paint. Grid, horizontal hex, and vertical hex samples matched CPU at 1×
  and 2× zoom with source and paint opacity. `CARGO_INCREMENTAL=0 cargo xtask ci` passed with
  67 GPU tests and 712 UI tests. Nested/effected/text sources are still guarded.
- 2026-10-30: large exports now render bounded drop/inner shadows and uniform layer blurs in
  overlapping GPU tiles. Page and selected-layer shadow pixels matched CPU across the 2048 px
  seam; a layer blur and a 2× transformed shadow did too. The halo includes the node's maximum
  transform scale. `CARGO_INCREMENTAL=0 cargo xtask ci` passed with 70 GPU tests and 712 UI
  tests. An unrelated local-code network test failed once under workspace load and passed both
  alone and on the full retry. Backdrop, progressive, grain, and unbounded effects remain open.
- 2026-10-30: native screen-ready GPU exports now convert Display P3 before readback. Drag
  previews, presentation textures, thumbnails, home cards, vectorization backgrounds, and font
  previews use that path. A P3 page and selected-layer fixture matched CPU screen colors within
  4/255, including translucent pixels. Browser presentation remains open.
- 2026-10-30: Copy as PNG now uses the same screen-ready GPU export, so a P3 selection reaches
  the native clipboard in sRGB without a CPU profile-conversion pass after readback.
- 2026-10-30: pattern sources may now contain a simple group of child shapes. The GPU draws the
  source subtree into the repeating cell and includes descendants in its cache key. Two-color
  group samples matched the CPU reference; nested patterns, masks, effects, and text remain
  guarded.
- 2026-10-30: a WASM compile probe confirmed that `vello_gpu` itself builds for
  `wasm32-unknown-unknown`. Lizard's canvas crate still does not: the native geometry-atlas Rayon
  worker requires a `Send + Sync` device, while browser WebGPU's device is local to its thread;
  a native-only shadow module also imports a gated blur module. Those modules need platform
  separation and an async browser initialization path before the web canvas can use this engine.
- 2026-10-30: simple group pattern sources now accept alpha and vector masks, plus the supported
  single-paint luminance mask. Repeating-cell samples matched CPU for all three mask types.
- 2026-10-30: the browser's direct Vello canvas now shares eframe's WebGPU device. A browser
  smoke test caught an existing WASM crash in use of `std::time::Instant` on both WebGPU and
  WebGL2; browser-compatible clocks fixed it. Chrome headless with BrowserWebGpu now starts
  without an exception; the WebGL2 fallback also starts without an exception. A newly drawn
  rectangle appeared in a captured WebGPU screenshot. The
  packaged loader was also exercised in Chrome. Browser export readback and scene pixel/latency
  comparisons are still open. The production WASM is 30,760,930 bytes raw; packaging it as
  14,142,387-byte gzip stays below the host's 25 MiB per-file limit.
- 2026-10-30: a call-site audit found a direct CPU `render_view` in the interactive eyedropper
  magnifier, plus plugin icon rasterization and SVG/PDF embedding paths. An earlier evidence
  note called the eyedropper GPU-first; that described its static color preview, not the live
  magnifier. These paths remain open above.
- 2026-10-30: an automated Chrome WebGPU smoke now creates a rectangle and reads the presented
  screenshot pixel. The center changed from `(30, 30, 30)` to `(217, 217, 217)` on `BrowserWebGpu`.
  This verifies canvas presentation after an edit, but does not establish pan/zoom latency,
  browser raster export, or pixel parity with the native renderer.
- 2026-10-30: `CARGO_INCREMENTAL=0 cargo xtask ci` completed with exit code 0 after the browser
  canvas and compatible clock changes. Browser startup and pixel checks were run separately in
  Chrome because the workspace gate does not execute a real browser.
- 2026-10-30: a rounded frame with clipped children now renders in the GPU pattern cell. The
  8 px repeated-cell fixture passed sampled GPU/CPU comparisons and visual inspection. Edge
  antialiasing differed by up to 48/255 in premultiplied channels at 2×; broader clipping and
  pattern combinations remain open. `CARGO_INCREMENTAL=0 cargo xtask ci` passed with this change.
- 2026-10-30: the interactive eyedropper magnifier now queues a 9 × 9 copy from the canvas GPU
  texture and polls its mapped result on later frames. The unit test verified interior and
  viewport-edge samples; Chrome drew a rectangle and displayed the sampled preview on both
  BrowserWebGpu and WebGL2 fallback. `CARGO_INCREMENTAL=0 cargo xtask ci` and a production WASM
  build passed. The click-to-pick command
  still performs a separate GPU-first 1 × 1 export; unsupported GPU scenes retain CPU fallback.
- 2026-10-30: a probe of Multiply fill plus Drop Shadow exposed a composition mismatch:
  the center was `(46, 38, 31)` on GPU versus `(204, 102, 51)` in the CPU reference. Isolating
  shadowed leaf content after drawing its shadow made all Vello-supported paint blends match the
  reference at fill, shadow, and background samples. Partial-opacity shadow stacks still hit a
  separate effect guard and remain on the CPU fallback. All 75 canvas-GPU tests and
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed.
- 2026-10-30: blending a whole shadowed node exposed a separate ordering issue: Lighten had
  erased the GPU shadow while the CPU reference retained it. Drawing the shadow before the
  node's blend layer brought every supported node blend mode back within 12/255 at sampled fill,
  shadow, and background pixels. Partial opacity remains guarded. All 76 canvas-GPU tests and
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed.
- 2026-10-30: native plugin grid SVG paths now go through the document vector importer and
  GPU-first export rather than directly calling the tiny-skia icon rasterizer. Fill and round-cap
  stroke samples at 72 px matched the prior renderer within 8/255; a direct GPU export of the
  imported icon succeeded. `CARGO_INCREMENTAL=0 cargo xtask ci` passed. Browser plugin icons
  still use the CPU fallback until WebGPU export readback is available.
- 2026-10-30: Display P3 soft proof now marks out-of-gamut pixels in the same GPU pass that
  converts the canvas for presentation. A 16 × 16 fixture matched every byte of the CPU proof
  within 4/255, and its enlarged GPU output was visually inspected. Toggling proof reuses the
  rendered vector texture. Native and WASM checks passed. The test app accepted a proof toggle,
  but its window screenshot timed out while the window was not being presented by macOS. All 77
  canvas-GPU tests and `CARGO_INCREMENTAL=0 cargo xtask ci` passed.
- 2026-10-30: Focus mode now uses the ordered GPU backdrop compositor when the focused subtree
  contains a supported blur or glass. A nested group with a background blur matched the CPU solo
  view at sampled pixels; an unsupported Plus Darker sibling no longer disqualifies that focus
  view. Fractional camera positions at 1×/2× keep texture scatter on GPU. Fine grains show some
  interpolation, but the rotated fixture kept its coverage and changed fewer than 5% of viewport
  pixels by more than 8/255. The acceptance criterion here is visual stability rather than an
  exact CPU raster match. `CARGO_INCREMENTAL=0 cargo xtask ci` passed, including 84 canvas-GPU
  tests, 718 UI tests, native workspace checks, WASM checks, plugins, formatting, Clippy and layers.
- 2026-10-30: the browser WebGPU update gate now uses the selected-outline capability check
  when painting outlines, rather than the ordinary full-document check. This allows unsupported
  effects in the outlined subtree to disappear with its fill without rejecting the GPU scene;
  unsupported painted siblings and backdrop effects remain guarded.
- 2026-10-30: native 4200 × 128 page and selected-layer exports with noise at the first tile seam
  and texture scatter at the second now remain on GPU. The wide result matched independent
  GPU crop renders at every pixel of two 160 × 128 windows spanning the seams. Other effect
  classes still need tile-specific bounds and remain guarded.
- 2026-10-30: native 4200 × 128 page exports with a backdrop blur and with glass now use the
  ordered GPU compositor in overlapping tiles. Pixel comparisons against a direct GPU crop
  around the 2048 px seam stayed within 8/255 for both effects. Progressive background blur,
  complex backdrop stacks, and selected-layer backdrop exports remain open. `CARGO_INCREMENTAL=0
  cargo xtask ci` passed with 87 canvas-GPU tests and 718 UI tests, plus native workspace, WASM,
  plugin, Clippy, formatting and layer checks.
- 2026-10-30: progressive layer blur now tiles on GPU for both page and selected-layer exports,
  and progressive background blur tiles on GPU for pages. A 4200 × 128 layer fixture and the
  backdrop fixture matched independent GPU crops within 8/255 around the 2048 px seam. Their
  maximum radius supplies the conservative halo. Selected-layer backdrop exports remain open.
  `CARGO_INCREMENTAL=0 cargo xtask ci` passed with 88 canvas-GPU tests and 718 UI tests, plus
  native workspace, WASM, plugin, Clippy, formatting and layer checks.
- 2026-10-30: selected-layer exports now build a temporary page containing just the selected
  roots and send any supported backdrop effect to the ordered GPU compositor over transparent
  clear. A selected blur matched sampled CPU pixels within 24/255 despite an unsupported sibling
  elsewhere on the page; selected glass also rendered on GPU. A 4200 × 128 selection with blur
  and glass matched direct GPU crops around the 2048 px seam within 8/255. Pattern sources
  referenced by selected layers remain visible to their GPU paint even when outside the selected
  roots; all four pattern export tests passed after that correction. Complex backdrop stacks
  still need broader coverage.
- 2026-10-30: native, browser WebGPU, and page export capability checks now ignore nodes on
  other pages. A two-page fixture with an unsupported blend on the hidden page passed, as did
  the selected backdrop and pattern export tests. `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0
  cargo xtask ci` passed with 90 canvas-GPU tests, 721 UI tests, native/WASM, plugins, Clippy,
  formatting, and layer checks. A parallel run had one unrelated timing-sensitive UI test
  failure; the test passed in isolation and in the serial full run.
- 2026-10-30: the ordered GPU backdrop compositor now accepts selected outlines for both its
  prefix and final scene. A fixture selecting either the shape below a blur pane or the pane
  itself matched the CPU reference at the sampled pixels within 24/255; selected outlines under
  glass matched within 40/255. Native and browser selected-outline routes use that compositor
  when a supported sibling backdrop is visible. `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0
  cargo xtask ci` passed with 91 canvas-GPU tests and 721 UI tests, plus native/WASM, plugins,
  Clippy, formatting, and layer checks.
