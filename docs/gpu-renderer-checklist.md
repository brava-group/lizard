# GPU renderer migration checklist

The current target is GPU rasterization for the editor canvas, including interaction previews,
when a compatible GPU is available. The egui controls are outside this milestone. Raster exports
and secondary previews remain recorded below as follow-up work, not as blockers to the canvas
milestone. CPU rendering remains a fallback for unavailable devices and an oracle for parity
tests. SVG and PDF keep their native vector output. A box is complete only after native and WASM checks, visual verification of
content and effect order, and an interaction or export test. The GPU image may differ aesthetically
from the CPU reference; pixel comparisons detect missing content and severe artifacts rather than
require an exact match. Do not remove a capability guard merely to mark a box done.

## Remaining production CPU raster paths (code audit)

| Entry point | Current CPU work | GPU completion condition |
| --- | --- | --- |
| `crates/ui-egui/src/render_worker.rs` | Canvas fallback for remaining combined view modes, unsupported backdrop stacks, and other unsupported scenes | GPU scene handles each view mode and paint/effect combination; worker runs only without GPU |
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
  - [x] Draw Outline mode while focused on one subtree through native and browser WebGPU;
    sibling contours stay outside the focused scene.
  - [x] Combine full-page and focused Outline mode with Pixel Preview; native and browser WebGPU
    register nearest-neighbor presentation at the document-pixel scale.
  - [x] Draw Pixel Preview at the document pixel scale on GPU and register the presented texture
    with nearest-neighbor sampling on native and browser WebGPU.
  - [x] Combine Pixel Preview with a focused subtree on native and browser WebGPU; the focused
    scene keeps document-pixel resolution and nearest-neighbor sampling.
  - [x] Combine Pixel Preview with selected outlines on native and browser WebGPU, retaining
    the document-pixel scale and selected contours in paint order.
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
  - [x] Ignore unsupported paints and effects under hidden ancestors when choosing the GPU
    canvas; those descendants do not contribute pixels to the visible scene.
  - [x] Skip static GPU effect preparation under hidden ancestors and release retained grain
    textures when a parent becomes hidden. An isolated test covers single Blur, single
    Texture, and ordered Texture/Blur behind a toggled group, while a visible sibling stays cached.
  - [x] Combine selected outlines with supported sibling backdrop blur or glass on native and
    browser WebGPU, preserving paint order even when the selected contour is behind the pane.
  - [x] Combine Focus with selected outlines on native and browser WebGPU, with optional Pixel
    Preview; draw only the focused subtree and exclude its siblings from capability checks.
  - [ ] Move the remaining mixed view modes and unsupported backdrop stacks to GPU.
  - [x] Render one uniform background blur inside a translucent group with the GPU isolated
    group compositor. The 25%, 50%, and 75% opacity fixtures at 1× and 2× each stayed below
    1% pixels differing by more than 20/255 from the CPU reference, with painted siblings
    before and after the group. Glass and complex clips remain guarded.
  - [x] Composite that group over a transparent page without painting the backdrop twice.
    GPU page-export samples of premultiplied RGBA, including partially transparent layers
    behind the pane, stayed within 20/255 of the CPU reference.
  - [x] Render two ordered uniform background blurs inside the same translucent group on GPU.
    The second blur samples the first pane in the GPU prefix. Full-frame 1×/2× comparisons
    stayed below 1% pixels differing by more than 20/255; transparent page-export RGBA
    samples also matched within 20/255.
  - [ ] Replace `render_worker`'s CPU `render_view` path for outline and unsupported previews
    with a GPU scene or explicit GPU-free fallback.
- [ ] Keep pan, zoom, frame drag, scale, rotate, clone, and creation GPU-resident at interaction
  speeds for documents with mixed effects; check error paths and resize behavior.
  - [x] Prepare the composited drag background, moving layer, and zoom detail with GPU-first
    rendering, retaining the existing GPU texture composition on screen.
  - [x] Suppress the CPU creation preview when the current GPU texture already contains the new
    shape, avoiding a stale second raster over the scene.
  - [x] Skip the drag path's optional 10 ms CPU-worker wait when the current frame already has a
    GPU texture; keep the wait for scenes that still need the CPU fallback.
  - [ ] Remove CPU rendering from unsupported drag scenes and measure latency on mixed effects.
- [x] Remove the CPU atlas path from production after direct GPU covers its use cases. The
  reference implementation remains compiled only in native tests.

## Paints, effects, and composition

- [ ] Support image filters, diamond gradients, pattern fills, shader fills, per-paint blend modes,
  non-normal node blends, complex text style runs, and Display P3 without CPU rasterization.
  - [x] Apply the seven image adjustment controls in a WGSL compute pass after decoding.
  - [x] Render isolated noise on skewed shapes through the GPU local-coordinate pass; a skewed
    rectangle matched the CPU reference within the existing pixel tolerance.
  - [x] Apply partial node opacity after isolated noise on GPU for rotated shapes; tested 25%,
    50%, and 75% at 1× and 2× on a colored backdrop.
  - [x] Render texture scatter on rotated shapes at 1× and 2× by aligning GPU cache bounds to
    document pixels; clipped and unclipped rotated fixtures match the CPU reference.
  - [x] Keep texture scatter on GPU at 1× and 2× during fractional camera pan. The static cache
    interpolates fine grains slightly; a fine-grain rotated fixture retained shape coverage and
    stayed within a 5% viewport difference threshold at both zoom levels.
  - [x] Keep texture scatter on GPU at fractional zoom up to 2× by selecting the 1× or 2×
    precomputed texture. A flat and rotated fixture at 0.75×, 1.25×, and 1.5× retained shape
    coverage and stayed below a 5% viewport difference threshold.
  - [x] Apply partial node opacity after texture scatter on GPU for rotated shapes, including
    both the 1× and 2× caches; tested 25%, 50%, and 75% opacity on a colored backdrop.
  - [x] Generate diamond gradients on the GPU, including individually painted vector regions.
  - [x] Generate repeating grid and shifted-hex pattern cells on the GPU for simple leaf and
    group sources, including supported alpha, vector, and luminance masks. Retain a guarded
    fallback for nested patterns, unsupported masks, effects, and complex text sources.
  - [x] Render a clipped rounded frame used as a pattern source on the GPU; sampled corner and
    center pixels and visually compared the repeated tile against the CPU reference.
  - [x] Render a uniform-style text node as a repeating pattern source on GPU at 1× and 2×;
    rich text runs with unsupported paints remain guarded.
  - [x] Paint vector text glyphs and decorations by style run on GPU, including node default
    style fills and solid-color styled text pattern sources at 1× and 2×. Unsupported run paints
    still trigger the CPU fallback; color glyph ink needs separate coverage.
  - [x] Render a repeating pattern whose source has one uniform Layer Blur on GPU at 1× and 2×;
    progressive blur and other source effects remain guarded.
  - [x] Render pattern sources with two or three uniform Layer Blurs on GPU in effect order.
    Repeated cells matched the CPU reference at 1× and 2×; four blurs remain guarded after
    the 2× comparison exceeded the visual difference limit.
  - [x] Compose document shader fills directly from GPU textures, with zoom-aware resolution,
    live uniforms, opacity, and bounded replacement of edited textures.
  - [x] Keep native Display P3 export pixels in document space and convert the main native canvas
    to sRGB in a GPU pass.
  - [x] Convert native secondary previews and presentation textures from Display P3 on GPU
    before readback, retaining CPU conversion only for the fallback.
  - [ ] Check Display P3 conversion in the browser presentation path.
  - [x] Render the Vello-supported blend modes on simple nodes, effect-free groups, and individual paints.
  - [x] Isolate the content of an opaque shadowed leaf before applying paint blend modes; all
    Vello-supported modes match the CPU reference with a Drop Shadow.
  - [x] Keep all 16 supported paint blends with a Drop Shadow on GPU at 65% node opacity.
    Full-pixel comparison and fill, shadow, and background samples passed; the older test's
    partial-opacity fallback assertion was stale after the shadow cache migration.
  - [x] Apply node blend modes to opaque leaf content after drawing its Drop Shadow; all
    Vello-supported modes match CPU samples at the fill, shadow, and background.
  - [x] Keep all 16 Vello-supported node blends with a single Drop Shadow on GPU at 25%, 50%,
    and 75% node opacity, compositing shadow and blended fill in separate opacity layers.
  - [x] Draw a single uniform Layer Blur with all 16 Vello-supported node blends directly on
    GPU at 100%, 25%, 50%, and 75% opacity, without a CPU or intermediate texture cache.
  - [x] Apply a single uniform Layer Blur to a leaf with two paints, where the second uses any
    of the 16 Vello-supported paint blends; isolate the paints before blurring at 100%, 25%,
    50%, and 75% node opacity.
  - [x] Combine one uniform Layer Blur with both a node blend and a second paint blend on GPU.
    Tested all 16 supported node modes paired with supported paint modes at full and half
    opacity, 1× and 2× zoom; all 64 comparisons stayed within the visual difference gates.
  - [x] Compose a second paint using any of the 16 Vello-supported blend modes inside an
    ordered Texture/Blur GPU cache, with or without Drop Shadow and at full or half node
    opacity. All 256 comparisons across both effect orders and 1×/2× passed; the largest
    shadow-first differences stayed under the visual gate with no severe pixels.
  - [x] Apply 12 verified Vello node blends to the GPU-cached `Layer Blur ↔ Noise` stack on
    opaque rotated leaves: Darken, Multiply, Color Burn, Screen, Plus Lighter, Overlay, Soft Light,
    Exclusion, Hue, Saturation, Color, and Luminosity. The cached source is rendered without its
    node blend, then blended over the page. Lighten, Color Dodge, Hard Light, and Difference remain
    guarded for this effect stack after a pixel probe found larger differences.
  - [x] Keep ten verified node blends with partial opacity on GPU for both orders of one Layer
    Blur and one Noise effect on rotated leaves: Darken, Multiply, Screen, Overlay, Soft Light,
    Exclusion, Hue, Saturation, Color, and Luminosity. Color Burn and Plus Lighter remain guarded
    at partial opacity; tested 25%, 50%, and 75% at 1× and 2×.
  - [x] Keep the same 12 opaque and ten half-opaque node blends on GPU for both Texture/Blur
    orders, with and without an ordered Drop Shadow on a rotated leaf. The 176 supported
    GPU/reference comparisons at 1× and 2× passed the visual gates; Color Burn and Plus
    Lighter at half opacity remain guarded.
  - [ ] Implement Plus Darker and blends combined with effect stacks.
- [ ] Support alpha and luminance masks, nested masks, clipped groups, and their effect stacks.
  - [x] Keep an alpha mask around a nested luminance mask on GPU, including a rotated parent
    group. Full-frame CPU comparisons at 1× and 2× found only 3 and 6 changed pixels.
  - [x] Keep a cached Drop Shadow on content inside nested alpha/luminance masks. The test
    verifies that the shadow changes more than 600 pixels at 1× and 2,400 at 2×, while GPU
    comparisons differ from the CPU by at most five pixels above 8/255.
  - [x] Compose translucent luminance gradient masks with the GPU destination-in pass;
    sampled reveal levels remain within 16/255 of the reference.
  - [x] Compose a single solid or gradient luminance stroke as the mask source on GPU;
    sampled edges and empty interior remain within 16/255 of the reference.
  - [x] Compose uniform-style text luminance masks with one solid or gradient paint on GPU;
    rich text run colors are converted separately in the following case.
  - [x] Compose styled text luminance masks on GPU by converting each run's solid or gradient
    paint to luminance alpha before the destination-in pass. A two-color text mask differed in
    only 27 pixels above 16/255 in the 200×100 comparison.
- [x] Keep composited inner and drop shadows on rotated groups in the GPU cache for tested spreads
  through ±32 document units; unsupported descendants and larger effects remain guarded.
  - [x] Cache individual drop and inner shadows on partially opaque rounded rectangles; apply
    25%, 50%, or 75% node opacity during GPU composition at 1× and 2×.
  - [x] Cache individual drop and inner shadows on partially opaque rotated groups containing
    multiple children and an alpha mask at 1× and 2×.
- [ ] Support arbitrary ordered combinations of layer/background blur, noise, texture, shadows,
  and glass on rotated/skewed shapes and groups, including multiple shadows.
  - [x] Support `Noise → Layer Blur` on rotated shapes at 1× and 2× zoom with GPU-only passes.
  - [x] Keep `Layer Blur → Noise` on rotated shapes on GPU. Its grain samples differ from CPU,
    but the contour, blur, and effect order remain intact at 1× and 2×.
  - [x] Keep longer ordered Layer Blur/Noise stacks on rotated shapes on GPU. `Blur → Noise →
    Blur`, `Noise → Blur → Noise`, and `Blur → Noise → Blur → Noise` retain the contour and order
    at 1× and 2×; the first matched the CPU fixture exactly and the others had only a few
    severe pixel differences.
  - [x] Apply partial node opacity after the cached Blur/Noise stack on rotated leaves, for both
    two-effect orders and Blur → Noise → Blur; tested 25%, 50%, and 75% opacity at 1× and 2×.
  - [x] Cache a single uniform Layer Blur on GPU for rotated leaves with 25%, 50%, and 75%
    opacity, applying opacity after the blur at 1× and 2×.
  - [x] Cache a single progressive Layer Blur on GPU with 25%, 50%, and 75% opacity at 1×
    and 2×, applying opacity after the variable-radius pass.
  - [x] Keep all six orders of one Layer Blur, one Noise, and one Drop Shadow on GPU at 1× and
    2× for rotated shapes and groups, including a group with a masked child. Each pass now runs
    at its position in the effect list rather than moving the shadow to the end.
  - [x] Keep those six effect orders on GPU for rotated leaves with 25%, 50%, and 75% node
    opacity, applying opacity after the ordered stack.
  - [x] Keep those same six orders on GPU for a rotated group with multiple painted children and
    an alpha mask at 25%, 50%, and 75% group opacity.
  - [x] Run both Layer Blur → Texture and Texture → Layer Blur on GPU for a rotated rectangle
    at 100%, 25%, 50%, and 75% node opacity. The ordered compute passes matched the reference
    exactly at 2× and had at most 1,094 changed pixels (four severe) at 1× in the 260×240 view.
  - [x] Keep Texture ↔ Noise in both orders on GPU at those four opacities. A document-scale
    1× cache avoids resampling the 2× scatter during zoom; 32 ordered comparisons across
    Blur/Texture and Noise/Texture now differ by at most 40 pixels, with no severe differences.
  - [x] Compose all six orders of one Layer Blur, one Noise, and one Texture on GPU at 100%,
    25%, 50%, and 75% opacity. Each effect visibly changes the reference image; all 48
    comparisons at 1× and 2× matched the CPU pixels within the 8/255 threshold.
  - [x] Compose Texture with one Drop Shadow in both orders, and every three-effect order
    containing Texture, Drop Shadow, and either Layer Blur or Noise. A rotated rectangle at
    four node opacities and 1×/2× passed 112 GPU/reference comparisons. The largest
    shadow-first difference was 5,159 pixels above 8/255 in a 520×480 image; at most two
    pixels per image differed by more than 48/255.
  - [x] Exercise all 24 orders of Layer Blur, Noise, Texture, and Drop Shadow on the same
    rotated shape at four opacities and 1×/2×. All 192 additional comparisons kept the
    effects in GPU order and passed the visual difference gates.
  - [x] Remove the four-effect count limit for the supported ordered Texture stack. Three
    additional five- and six-effect stacks, including repeated Blur and Noise passes, passed
    24 GPU/reference comparisons at four opacities and 1×/2×; the highest pixel difference
    was 7,983 of 249,600 pixels above 8/255, with one pixel above 48/255.
  - [x] Compose Texture with two distinct Drop Shadows in every three-effect order, plus a
    five-effect Blur/Texture/Shadow/Noise/Shadow stack. All 32 GPU/reference comparisons at
    four opacities and 1×/2× passed; shadow-first texture scatter differed by at most 9,459
    pixels above 8/255 and 263 above 48/255 in a 520×480 image.
  - [x] Cache ordered Texture/Blur, Shadow/Texture/Noise, and Blur/Noise/Texture/Shadow on GPU
    for a rotated group with painted children and an alpha mask. Eighteen comparisons at
    25%, 50%, and 75% group opacity and 1×/2× passed; the largest texture case differed by
    1,389 pixels above 8/255 with no severe pixels.
  - [x] Cache Texture/Blur and Blur/Noise/Texture on GPU for a rotated, partly transparent
    vector. All eight 1×/2× comparisons at full and half node opacity matched the CPU
    reference within 8/255, and each effect stack visibly changed the source vector.
  - [x] Apply Drop Shadow spread in ordered GPU stacks. Positive spread uses the vector
    silhouette for shapes; group content uses GPU morphology, and negative spread erodes the
    alpha. Texture stacks passed 40 comparisons at 1×/2× across ±8, +16, and ±32 spread
    with four node opacities. Blur/Noise/Shadow passed 12 comparisons across ±8 spread, and
    a rotated masked group with Texture and +8 spread passed six more.
- [ ] Remove shape, spread, stroke, size, zoom, and region-of-interest limits in GPU effect passes
  without exceeding device texture limits; tile large surfaces instead.
- [ ] Measure visual correctness and latency on the icon, a complex multi-frame document, UltraWide/Retina,
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
  - [x] Choose the tile core from the effect halo instead of imposing a 512 px halo cap.
    A 4200 × 128 page with a 90 px layer blur used a halo over 512 px on GPU; samples
    around the 2048 px seam matched an independently rendered GPU crop within 12/255.
    Effects requiring 2048 px or more of halo still need a separate large-surface strategy.
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

- 2026-10-30: 128 GPU/reference comparisons covered two paints on one leaf, all 16 Vello
  paint blends for the second paint, a uniform Layer Blur, four node opacities, and 1×/2×.
  Every pixel matched. The paint blend now sees earlier paints of its leaf inside an isolated
  layer before the blur is applied; Plus Darker remains guarded.

- 2026-10-30: a 128-case matrix (16 node blends × four opacities × two zoom levels) for a
  single uniform Layer Blur over a colored backdrop passed GPU/reference comparisons. At most
  48/249,600 pixels differed above 8/255, with none above 48/255. Vello's filter and blend
  layers draw this case directly on GPU; Plus Darker remains guarded.

- 2026-10-30: a repeating pattern sourced from a uniformly blurred vector rectangle now
  renders through Vello's GPU filter. The complete 64×48 pattern export matched the CPU
  reference pixel for pixel at 1× and 2×. Pattern sources with progressive blur or other
  unsupported effects remain guarded.

- 2026-10-30: 120 GPU/reference comparisons covered ten cached Blur/Noise node blends at
  25%, 50%, and 75% opacity, both effect orders, and 1×/2×. The worst retained case differed
  at 21,383/228,800 pixels above 8/255 and none above 48/255. Color Burn and Plus Lighter
  exceeded the 10% changed-pixel gate in some Blur → Noise cases, so partial opacity for those
  modes remains guarded. The probe also caught and fixed a regression in cache preparation:
  blended sources must retain their opacity, while normal-blend sources apply opacity after
  effects.

- 2026-10-30: the split shadow/fill opacity path now covers all 16 Vello-supported node blend
  modes. A 96-case GPU/reference matrix (16 modes × three opacities × two zoom levels) passed
  at 1× and 2× on a colored backdrop. Plus Darker remains unsupported by Vello and guarded.

- 2026-10-30: a translucent rounded rectangle with Multiply or Screen and one Drop Shadow
  now stays on GPU. Twelve GPU/reference comparisons at 25%, 50%, and 75% node opacity and
  1×/2× differed at most 132/249,600 pixels above 8/255 and none above 48/255. The first
  probe exposed a full-shape error because the blend sampled inside the shadow's opacity
  layer; separating shadow and fill opacity removed that error.

- 2026-10-30: partially opaque rotated groups with three painted children and an alpha mask
  now cache a single Drop Shadow or Inner Shadow on GPU. Twelve GPU/reference comparisons at
  25%, 50%, and 75% opacity and 1×/2× differed at most 330/312,000 pixels above 8/255 and
  none above 48/255. Group opacity is applied after the cached shadow and child composition.

- 2026-10-30: individual drop and inner shadows on partially opaque rounded rectangles now
  use GPU caches. Twelve GPU/reference comparisons at 25%, 50%, and 75% opacity and 1×/2×
  differed at most 132/249,600 pixels above 8/255 and none above 48/255. The capability
  remains limited to rectangles and frames; other translucent shapes retain their guards.

- 2026-10-30: the progressive Layer Blur fixture now covers 100%, 25%, 50%, and 75% node
  opacity at 1× and 2×. All eight GPU/reference comparisons passed the existing 0.1% pixel
  tolerance while using a cached GPU result. The partial-opacity variants no longer fail the
  progressive blur capability check.

- 2026-10-30: six GPU/reference comparisons for one uniform Layer Blur on a rotated
  partially opaque rectangle over a colored backdrop matched pixel for pixel at 25%, 50%, and
  75% opacity at 1×/2×. The blur is prepared once in a GPU texture and node opacity is applied
  during final composition, allowing reuse during pan and zoom.

- 2026-10-30: 36 GPU/reference comparisons covered all six Blur/Noise/Shadow orders on a
  rotated group with three visible child shapes, one alpha mask, and 25%, 50%, or 75% group
  opacity at 1×/2×. The worst order differed at 12,359/312,000 pixels above 8/255 and none
  above 48/255; three orders matched exactly. The default Pass Through group blend now keeps
  its post-effect opacity layer in the cached GPU path.

- 2026-10-30: 36 GPU/reference comparisons covered all six orders of one Layer Blur, one
  Noise, and one Drop Shadow on a rotated partially opaque leaf, at 25%, 50%, and 75% node
  opacity and 1×/2×. Three orders matched pixel for pixel. The worst order differed at
  10,527/249,600 pixels above 8/255 and none above 48/255. Group opacity with this stack
  remains separately guarded.

- 2026-10-30: 18 GPU/reference comparisons for partially opaque rotated Blur/Noise stacks
  passed at 1× and 2×. Noise → Blur and Blur → Noise → Blur matched pixel for pixel; Blur →
  Noise differed at most 10,479/249,600 pixels above 8/255 and none above 48/255. The GPU
  cache now applies normal node opacity once, after the ordered effect stack. Shadow-containing
  stacks at partial opacity remain guarded.

- 2026-10-30: a rotated rectangle with one Texture effect and partial node opacity now stays
  on GPU. The unattenuated source feeds both scatter caches; final composition applies opacity
  once. All six GPU/reference comparisons at 1× and 2× matched pixel for pixel at 25%, 50%,
  and 75% opacity. Opaque texture preparation continues without cloning the document.

- 2026-10-30: a translucent rotated rectangle with one Noise effect now remains in the GPU
  canvas. The source stays opaque in the noise cache and its node opacity is applied when the
  cached texture is composited. Six GPU/reference comparisons at 1× and 2× found at most
  12,169/211,200 pixels different by more than 8/255 and none by more than 48/255. The
  original pre-noise opacity path differed at 19,935/211,200 pixels at 25% opacity. Opaque
  noise cache preparation still uses the original document without a full clone.

- 2026-10-30: partial-opacity Multiply and Screen on rotated leaves with cached Blur/Noise
  passed 24 isolated GPU/reference comparisons at 1× and 2×. The largest difference above
  8/255 was 21,383/228,800 pixels (Screen at 50% opacity with Blur before Noise, 2×); no
  pixel differed above 48/255. I inspected both rendered images: the rotated contour,
  transparency, blur, and noise remain present. Other cached blend modes and partial-opacity
  shadow stacks remain guarded.

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
- 2026-10-30: inspected CPU/GPU images for a rotated `Layer Blur → Noise` rectangle. GPU grain
  looked finer without losing the blurred contour. At 1×, 3868 of 78000 pixels differed by more
  than 8/255, but only 6 differed by more than 48/255; at 2× the >8/255 count was 12950 of
  312000. The capability guard now accepts that two-effect order, with a regression test limiting
  large differences rather than requiring identical random grain samples. Longer untested
  rotated stacks remain guarded. All 91 GPU tests passed, and `RUST_TEST_THREADS=1
  CARGO_INCREMENTAL=0 cargo xtask ci` passed across native/WASM, plugins, Clippy, formatting,
  and layer checks.
- 2026-10-30: luminance masks with translucent linear-gradient stops now use the GPU mask
  composition. Five samples across the transition matched the CPU reference within 16/255;
  the existing opaque-gradient and alpha/vector-mask cases remained covered. A second fixture
  with a stroke-only luminance mask passed for both a translucent solid stroke and a translucent
  gradient stroke, preserving its empty interior and border pixels. All 92 canvas-GPU tests
  passed, and `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci` passed across
  native/WASM, plugins, Clippy, formatting, and layer checks.
- 2026-10-30: texture scatter no longer forces a CPU frame at fractional zoom up to 2×. The
  1× cache serves scales through 1× and the 2× cache serves larger scales. At 0.75×, 1.25×,
  and 1.5×, the flat fixture differed from CPU at 405/23220, 808/64575, and 730/93150
  pixels by more than 8/255; the rotated fixture differed at 422/23220, 669/64575, and
  921/93150. The shape remained covered in each case. Zoom above 2× remains guarded until
  the texture cache can be rendered at the required resolution. All 93 canvas-GPU tests passed,
  and `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci` passed across native/WASM,
  plugins, Clippy, formatting, and layer checks.
- 2026-10-30: uniform-style text with a single visible solid or gradient paint now serves as
  a luminance mask on GPU. In a 200 × 100 fixture, the solid mask revealed 1915 glyph pixels
  and differed from CPU by more than 16/255 at 43 pixels; the gradient revealed 1873 and
  differed at 41. Text with per-run styles still uses the guarded CPU fallback. All 94
  canvas-GPU tests passed, and `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci`
  passed across native/WASM, plugins, Clippy, formatting, and layer checks.
- 2026-10-30: a uniform-style letter tile now renders through the GPU pattern cache. CPU and GPU
  both covered 264 opaque pixels at 1×; at 2× they covered 656 and 736. Visual inspection showed
  the same glyph grid with softer GPU edges; no channel differed by more than 32/255. The
  16/255 difference counts were 132/3072 pixels at 1× and 584/12288 at 2×. Rich text runs
  still use fallback because separate glyph styles are not yet represented in the cache. All
  95 canvas-GPU tests passed, and `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci`
  passed across native/WASM, plugins, Clippy, formatting, and layer checks.
- 2026-10-30: Outline plus Focus now routes through one GPU outline scene containing only the
  focused subtree. A 100 × 50 GPU fixture retained the focused rectangle contour and omitted
  its sibling. The UI dispatch test confirmed the focused backend call, and the native/browser
  backends track focus in their outline texture cache keys. I inspected the generated GPU PNG:
  it showed the focused rectangle only. All 96 canvas-GPU tests passed; 719 UI tests passed and
  3 were ignored. `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci` passed across native/WASM,
  plugins, Clippy, formatting, and layer checks.
- 2026-10-30: Focus plus Pixel Preview now routes through the existing GPU focus scene with
  pixel-scale rendering and nearest-neighbor presentation on native and browser WebGPU. The UI
  dispatch fixture confirmed a focused rectangle reached the backend at 1× document scale
  while the editor view was zoomed to 8×. All 96 canvas-GPU tests passed; 720 UI tests passed
  and 3 were ignored. `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci` passed across
  native/WASM, plugins, Clippy, formatting, and layer checks.
- 2026-10-30: Selected Outline plus Pixel Preview now shares the GPU selected-contour scene and
  nearest-neighbor presentation. The UI dispatch fixture confirmed a selected rectangle reached
  the GPU backend at 1× document scale while the editor view was zoomed to 8×. All 96 canvas-GPU
  tests passed; 721 UI tests passed and 3 were ignored. `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0
  cargo xtask ci` passed across native/WASM, plugins, Clippy, formatting, and layer checks.
- 2026-10-30: Focus plus selected outlines now uses the focused-root GPU scene on native and
  browser WebGPU, with the ordered backdrop compositor retained for a blur inside focus. The
  focused capability check skips unsupported paint in outside siblings and selected contours.
  The UI dispatch test covered normal and 1× document pixel scale; the GPU fixture matched the
  focused CPU reference at selected contour, filled sibling, and backdrop samples. I inspected
  the generated GPU image: one outlined rectangle, one filled blue sibling, no outside layer.
  All 98 canvas-GPU tests passed; 722 UI tests passed and 3 were ignored. `RUST_TEST_THREADS=1
  CARGO_INCREMENTAL=0 cargo xtask ci` passed across native/WASM, plugins, Clippy, formatting,
  and layer checks.
- 2026-10-30: full-page and focused Outline mode now register nearest-neighbor presentation
  when Pixel Preview is active in native and browser WebGPU. Switching that flag re-registers
  the texture even when the vector scene itself is unchanged. UI tests covered both modes at
  normal scale and 1× document pixel scale under 8× editor zoom. All 98 canvas-GPU tests passed;
  722 UI tests passed and 3 were ignored. `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci`
  passed across native/WASM, plugins, Clippy, formatting, and layer checks.
- 2026-10-30: a rotated 120 × 96 rectangle with longer Blur/Noise stacks now uses the ordered
  GPU effect cache at 1× and 2×. For `Blur → Noise → Blur`, CPU/GPU RGB samples were identical
  across 78,000 and 312,000 pixels. The two other tested orders differed above 8/255 at
  4,369–4,404 pixels of 78,000 at 1× and 14,725–14,814 of 312,000 at 2×; at most five pixels
  differed above 48/255. I inspected the GPU and CPU images; both retained the same rotated
  contour, blur, and visible grain. In two local 30-position pans at 600 × 520, 2×, the cached
  GPU path prepared once in 12.9–14.7 ms, then rendered at 1.08–1.48 ms p50 and 7.00–8.44 ms
  p95. A full CPU `render_view` measured 4.43–4.46 ms p50 and 5.10–5.39 ms p95. This is a
  small-scene comparison against full CPU rendering, not the editor's retained CPU worker. All
  99 canvas-GPU tests passed; 722 UI tests passed and 3 were ignored. `RUST_TEST_THREADS=1
  CARGO_INCREMENTAL=0 cargo xtask ci` passed across native/WASM, plugins, Clippy, formatting,
  and layer checks.
- 2026-10-30: the saved `Lizard-new-icon.figcraft` renders through GPU at 1× and 2×, including
  its drop shadow, inner shadow, and layer blur. I inspected the GPU and CPU images; the icon,
  hole, gradient, edges, and shadow remain visually intact. Mean RGB difference was 0.798/255
  at 1× and 0.764/255 at 2×; 0.601% and 0.401% of pixels differed above 8/255. The 1× first
  preparation took 826.82 ms, while the subsequent 2× preparation took 29.44 ms. GPU first
  submissions took 4.89 ms and 1.55 ms; warm submissions took 0.94 ms and 1.29 ms, compared
  with 107.83 ms and 311.57 ms for full CPU reference renders. Submission time does not include
  presentation latency, and this icon has no glass effect. The cold preparation cost needs its
  own interaction measurement before declaring the complete canvas fast.
- 2026-10-30: when a drag frame already has a GPU texture, the editor no longer waits up to 10 ms
  for a pending CPU-worker frame. The GPU-first drag and resize tests passed. The full
  `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 cargo xtask ci` gate passed with 99 canvas-GPU tests
  and 722 UI tests passing (3 ignored). This removes a possible main-thread wait; end-to-end
  drag latency still needs a mixed-effects measurement.
- 2026-10-30: the GPU compositor now applies the drop shadow at its place in the ordered effect
  stack. Six permutations of Blur, Noise, and Shadow passed at 1× and 2× for a rotated rectangle
  and a rotated group with two painted children and a mask. `Noise → Blur → Shadow` matched all
  RGB samples in both fixtures; other orders changed up to 18,331/312,000 pixels above 8/255
  for the rectangle and 14,988/312,000 for the group. At most 87 rectangle pixels and 37 group
  pixels differed above 48/255. I inspected the GPU and CPU images of the worst group case;
  content, mask, shadow, blur, and grain remain present. An existing axis-aligned group test now
  allows a 2% difference above 8/255 for early-shadow orders; its observed maximum was
  1,947/201,600 pixels, with no differences above 48/255.
- 2026-10-30: rotated opaque leaves using Multiply or Screen with either `Layer Blur → Noise`
  or `Noise → Layer Blur` now stay on GPU. At 1× and 2×, `Noise → Blur` matched the CPU reference
  exactly. The largest `Blur → Noise` difference above 8/255 was 10,713/228,800 pixels for
  Screen at 2×; no pixel differed above 48/255. I inspected the GPU and CPU images for that
  case: the backdrop color, rotated contour, blur, and grain are intact. This initial check did
  not cover the additional blends tested below or partial opacity with cached effect stacks.
- 2026-10-30: an isolated 16-mode probe expanded the cached Blur/Noise leaf path to 12 Vello
  blends listed above at 1× and 2×. Every retained mode stayed below 8% of pixels differing by
  more than 8/255 and below 1% differing by more than 48/255. Lighten, Color Dodge, Hard Light,
  and Difference exceeded those limits in at least one `Blur → Noise` case, so their GPU
  capability remains off for this stack. The isolated regression checks all 12 retained modes
  and asserts the other four still request the fallback.
- 2026-10-30: the canvas capability scan now skips unsupported descendants of hidden groups.
  An isolated test toggled the parent visibility for both a Plus Darker child and a child with
  an unsupported shadow blend, and compared the hidden scene's center pixel against the CPU
  reference. Visible unsupported descendants still require the guarded fallback.
