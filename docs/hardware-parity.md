# Hardware and platform parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-21 · **Change:** minor (cached 512 px wide-canvas regions and native-resolution zoom fill; measured ultrawide Retina canvas and idle frame drag preparation; native canvas cursors and page-attached agent presence; merged upstream 0.7.0 updates; retained wide-view zoom-out coverage; bounded frame drag previews at high zoom and large window sizes; native composited drags for eligible topmost frames; measured pixel-wheel zoom path on macOS/browser; sharper zoom previews and same-frame vector updates for cheap drags; localized canvas texture uploads; copy and paste in the browser; Windows QA pass; Display P3: gamut warning and soft-proof (true P3 blocked upstream); several windows; pen pressure for the Pencil and brushes on Windows and from browser styluses) · **Target:** Figma Design (desktop app 126.10.7)

How Lizard uses hardware and which platforms it runs on, against Figma. Figma's desktop app is
Electron with a GPU helper process; its canvas renders on the GPU (WebGL, with WebGPU rolling out
since 2025). Lizard renders the canvas on the CPU and presents it through wgpu.

**Summary (estimated):** hardware ~60%, performance ~60%, platforms ~75%.

## Hardware

| Feature | Figma | Lizard (macOS / Windows / Linux / web) | Hours |
|---|---|---|---|
| Canvas rendering | GPU | CPU (tiny-skia), banded across cores; wide viewports use cached 512 px region textures composed by wgpu, while smaller ones retain a whole-view image with damaged-region uploads; levels of detail below 50% zoom | 30–60 for a GPU vector path |
| Effects (blurs, glass, noise) | GPU | CPU | in the above |
| HiDPI / Retina | yes | yes (all); a canvas image rendered at the screen's density is placed on whole device pixels, so fractional pans, zooms and display scales don't blur it (#359; geometry tests, the snapping not judged by eye) | — |
| Wide-gamut display (Display P3) | yes, on capable displays | colours stored and exported in P3; the screen shows sRGB: eframe/egui-wgpu 0.36 configure an 8-bit sRGB surface with no colour space option, although wgpu 30 can present Display P3 or extended sRGB on Metal, DX12, Vulkan and WebGPU (see gaps #18). The colour picker warns about colours outside sRGB and View ▸ Soft-proof sRGB stripes them on the canvas | 2–5 |
| Trackpad pinch zoom and two-finger pan | yes | yes (raw pixel-wheel zoom on macOS/browser; explicit pinch events and smooth pan) | — |
| Pen tablet pressure (pencil, Figma Draw brushes) | yes (forum reports of regressions in 2025) | Windows pens (winit pointer input) and browser styluses that send touch events with a force (an Apple Pencil in Safari); the Pencil records it as the stroke's width profile. A mouse, macOS and Linux tablets and pens sending only pointer events in a browser report none: constant width | 1–3 |
| Several windows / multi-monitor | yes (a window per file; presenter in its own window) | desktop: File ▸ New window, a tab's Open in new window or a tab dragged off the strip; each window has its own tabs, panels and canvas, draws only when it needs to and asks about unsaved changes when it closes. Only the main window has a screen-reader tree (eframe 0.36), so files stay tabs while a screen reader runs; positions aren't restored at the next start; the presenter stays inside its window; one window in the browser | 1–2 |
| Screen readers | partial | AccessKit tree on desktop (names, roles, states), never tested with a real screen reader; none in the browser | 4–8 + human QA |
| Keyboard layouts (non-US) | yes | keymap uses logical keys; not checked on AZERTY/QWERTZ/JIS | 1–2 |

## Performance (measured by `cargo xtask bench`, synthetic files, owner's Mac)

| Case | Lizard | Figma |
|---|---|---|
| 50k-layer page, zoomed out, full render | ~8 ms (levels of detail) | not timed side by side |
| 10k layers: full 1440×900 render | 7–15 ms | not timed |
| 10k layers: move one layer, re-render | ~0.2 ms | not timed |
| Design system of ~300 components × 2,000 instances: recolour a main used ~900 times | 4–18 ms | not timed |
| `.figcraft` at 50k layers: save / load | ~0.09 s / ~0.16 s | n/a |
| Idle window | 0 frames, ~0% CPU | — |

In a 2880×1800 screenshot session on the owner's Mac, moving a 100×70 rectangle in 20-unit
steps through `node.set` uploaded 34,364 pixels per settled edit, versus 3,791,886 for the
initial full texture. This measures conversion and upload area, not GPU path drawing or pointer
latency; the test document is small.

With the synthetic BetterDeck design (248 nodes: 127 frames, 109 text layers, shapes and nested
auto layout), dragging its entire 2048×1210 frame in 10 rapid, alternating moves over 900 page
units took a median 65 ms for the last pointer move to reach the canvas texture before the
multi-base patch, and 53 ms after it (three runs each, 2880×1800 Retina desktop session).
The final update transferred about 1.33 million pixels instead of the full 3.79 million.
Settled moves of 20–900 page units took a median 53 ms from pointer request to current texture,
versus about 66 ms before text-path caching and direct patch copying. These timings end when egui
receives the texture; GPU upload and display presentation come later. This remains the path for
frames that cannot be composited independently. A native drag of a single, topmost sRGB frame
without backdrop-dependent effects now prepares a separate frame layer and background once, then
translates that layer with the pointer while the vector document updates. On the 248-node
BetterDeck document, preparing both layers took 33 ms in one release build run; subsequent control
requests took about 12 ms median, including UI and control overhead. Screen presentation latency
and a direct comparison with Figma were not measured. At 800% zoom the original full-frame
preparation cost about 357 ms for a 248-node BetterDeck wrapper. A bounded preview plus a sharp
visible tile, skipping the background render when the frame is the only page child, prepared in
about 40 ms in the same release setup. The former 8-million-pixel canvas cutoff is removed;
with other page siblings a bounded background preview is rendered. The vector render returns
after release.

On the owner's 5120×2160, 75 Hz ultrawide (2× Retina), expanding a 1440×900 window to
2560×1050 points increased the visible canvas from about 2.49 million to 7.45 million device
pixels. The BetterDeck full canvas texture grew from about 3.79 million to 8.99 million pixels;
one settled wide full render reached the UI in 77–114 ms. A selected top-level BetterDeck frame
prepared its composited drag layer in 35–48 ms in the `dev` build. The layer now prepares while
the selection is idle: an inspected drag entered composited mode on its first movement instead of
waiting for preparation during the gesture. Native canvas cursors now move independently of
egui's frame rate. These are control-channel and render measurements, not physical
cursor-to-display latency. The brief stretched window during macOS titlebar expansion precedes
Lizard's re-layout; the wide full render and GPU presentation remain measurable limits.

In the same ultrawide window, 512 px page-aligned region textures replaced the single canvas
texture when the request exceeded four million device pixels. The first view uploaded 45 regions
(11.8 million pixels including cell edges, versus 8.99 million in the former whole texture) and
rendered in about 71 ms in the `dev` build. A 100% zoom of BetterDeck rendered 40 regions in about
63 ms after parallelization (186 ms when those regions were serial). A 150-unit pan at that zoom
rendered and uploaded one new five-region column (1.31 million pixels) in about 1.5 ms. Tile
overdraw makes the first upload larger; the benefit is reuse after pan and localized edits. A
whole-frame drag at 400% with only the top-left corner visible stayed in composited mode at
1.3–1.6 ms inspected UI frames after 111 ms of idle preparation. The window control channel does
not measure GPU presentation or physical cursor-to-display latency.

A second synthetic dashboard benchmark (142 nested frame, shape and text nodes) at 2880×1800
rendered 10 alternating large moves with a median of 16.9–18.6 ms for vector rendering plus
texture conversion across runs; document edits took under 0.1 ms. In the 1440×900 headless editor,
one run whose last render took 7.9 ms showed the current vector texture in all 10 input frames
(5.1 ms median input frame time); another run stayed asynchronous for all 10 frames when its
previous render exceeded the 10 ms fast-path threshold. These are CPU and egui texture timings,
not measured screen presentation. Large or slow renders can still trail the pointer.
During a smooth zoom, a covering full-resolution texture is kept until the new vector render is
ready. The UI no longer presents the worker's half-resolution preview. When a smaller viewport
zooms out beyond its cached image, it makes a native-resolution vector fill in the input frame;
wide viewports use the region worker. An old image can still look soft while it is being enlarged
before new vector pixels arrive, and a complex native-resolution fill can lengthen one input frame.
Continuous pinch/Cmd-scroll has 0.8× logarithmic gain, keeping reciprocal gestures symmetric.
The drag benchmark used Cargo's `dev` profile (`opt-level=1` for the workspace). In a separate
`release` run with the same synthetic dashboard plus an imported screenshot, the initial 2880×1800
full render took 388 ms. A control-channel jump from 200% to 25% produced a covering preview in
13.5 ms and a refined image in 86 ms. The screenshot after the jump showed the full view with no
empty strips. These are different operations and cannot be used as a dev-versus-release speedup
ratio. They show that expensive full CPU renders remain a limit even in an optimized build.

Nothing has been timed against Figma on the same document (needs `.fig` import). 

## Platforms

| Platform | Figma | Lizard | Notes |
|---|---|---|---|
| macOS | desktop app (Intel and Apple silicon) | universal `.dmg`, signed and notarized (v0.2.0) | where nearly all QA happens. Quitting ends the process once the preferences, recovery copies and live sessions are taken care of: the window's teardown crashed on Macs with a Touch Bar (#781); Quit from the menu bar, which asks nothing, keeps unsaved files' recovery copies for the next start |
| Windows x64 / arm64 | desktop app | `.msi` and portable zip for x64, x86 and arm64 | built in CI; Windows: QA pass done 2026-10-11, findings in PR |
| Linux | none (browser only) | AppImage, `.deb`, `.rpm`, Flatpak, tarball, x86_64 and aarch64; riscv64 tarball | CI tests run here. Clipboard (also the BSDs): reads run on a helper thread that gives up after 2 s, and images copied in other apps paste under Wayland through the data-control protocol (#693). Under Wayland the app can't tell its window is minimised, so control requests that need a drawn frame may still wait there (#720). None of this run on a Linux desktop here. On a Wayland compositor without server-side decorations (GNOME) the window gets winit's Adwaita controls (#592: checked by its author in a GNOME Wayland session; KDE not run) |
| FreeBSD | none | tarball | |
| Web | primary (figma.com) | `figcraft-web` (WASM, WebGPU/WebGL2), web zip | no accessibility tree; file access through the browser; copy and paste through the system clipboard (copies written from the copy event, pastes read from the paste event; no pasting images from other apps) |
| iOS / Android | apps: view, comment, mirror prototypes | none (the web build is not touch-first) | 20–40 h for a viewer |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | minor | Wide viewports now use cached 512 px textures composed by wgpu, with CPU rendering of new regions in parallel. On BetterDeck at 5120×2100 physical window pixels, 100% zoom refined in about 63 ms after parallelization versus 186 ms with serial regions; a 150-page-unit pan rasterized and uploaded one 1.31-million-pixel column in about 1.5 ms. A composited whole-frame drag with only its top-left corner visible showed 1.3–1.6 ms inspected UI frames after its idle preparation. These are control/render timings, not physical cursor latency. Native-resolution zoom fills replace the old half-resolution UI preview; full GPU vector rendering remains open. |
| 2026-10-21 | minor | Tested a real 5120×2160 ultrawide at 2× Retina with BetterDeck. The wide canvas requests about 3× the visible pixels of the initial window and a full texture of 8.99 million pixels. Eligible frame drag layers now prepare while selected and idle; the first inspected move used composition. Canvas tool cursors use native OS images rather than a painted overlay, and agent presence follows page coordinates with frame shimmer during tool work. GPU canvas work and estimates remain open. |
| 2026-10-18 | minor | macOS quit crash (#781): on Macs with a Touch Bar the app could crash as it quit (an exception in AppKit's Touch Bar support while the window's content view, which the accessibility adapter subclasses, was torn down). Quitting now keeps everything (preferences, this run's recovery copies, live sessions ended) and then ends the process before that teardown. Quit from the menu bar ends the app without the unsaved-changes question, so unsaved files' recovery copies now stay and reopen at the next start instead of being removed. Numbers unchanged |
| 2026-10-17 | minor | Zoom-out retains the broadest refined canvas texture as a one-view fallback and paints it under the current view only when new area is exposed. With a 2,880×1,740 desktop window, the 248-node BetterDeck scene's old synchronous preview took 4.7–6.4 ms at 800%→100%→25%, with refinement 17–38 ms. On a synthetic dark page with 1,005 nodes at four corners, the old 25% preview took 1.2–19.2 ms over 12 alternating runs; after warming the retained view, 12 runs each at 100% and 25% reported zero uncovered pixels in the first inspected frame and no synchronous preview. The inspected UI frames took 0.17–0.45 ms. Control inspection does not capture every presented frame or display latency; GPU rendering remains open. |
| 2026-10-17 | minor | High zoom frame dragging now uses a bounded moving preview plus a sharp visible tile and remains eligible when the canvas texture exceeds 8 million pixels. Pages with only the moving frame skip a background render; pages with siblings use a bounded background preview. On the 248-node BetterDeck wrapper at 800%, measured preparation fell from about 357 ms to about 40 ms. Ultra-wide screen presentation was not measured; estimates remain. |
| 2026-10-17 | minor | Native drags of eligible topmost frames composite a prepared frame layer over a background without that frame, following the pointer while the vector document changes. The 248-node BetterDeck release run prepared in 33 ms and 10 rapid control moves took about 12 ms median per request; pixel equivalence and drag/refinement tests pass. Unsupported frames and browser builds keep the existing render path; GPU canvas work and estimates remain. |
| 2026-10-17 | minor | Canvas zoom reads each modified pixel-wheel event before egui smoothing on macOS/browser; explicit pinch events retain their existing gain. Browser/platform gesture equivalence beyond the measured Chrome/macOS case remains unverified. |
| 2026-10-17 | minor | Smooth zoom keeps an existing full-resolution texture when it covers a nearby view; zoom-out that exposes uncached area renders a full-viewport half-resolution vector preview in the input frame. Stationary drags avoid redundant engine work. Cheap vector drags can finish in the pointer's UI frame with a 10 ms wait; heavier work remains off thread. The 142-node synthetic benchmark above records the practical limit; no GPU path or parity estimate change. |
| 2026-10-16 | minor | Same-view canvas edits and live drags convert and upload damaged pixel regions only, even when intermediate positions were skipped. Cached text outlines and a direct copy of rendered patch rows reduce CPU work. Pan, zoom, soft proof and changes that cannot safely use a patch send full textures. `ui.inspect.canvas.uploadPixels` and `textureCurrent` make the transfer observable. The document and undo remain vector based; GPU canvas rendering is still missing. |
| 2026-10-17 | minor | Linux row: clipboard reads on Linux and the BSDs time out after 2 s on a helper thread, images copied in other apps paste under Wayland (arboard's data-control feature, #693), a minimised window can't be detected under Wayland, so frame-only control requests may still wait there (#720), and Wayland compositors without server-side decorations give the window Adwaita controls (#592, checked by its author on GNOME, not by the maintainers; KDE not run); compiled in CI, not run on a Linux desktop. HiDPI row: native-density canvas images placed on whole device pixels (#359, geometry tests; not judged by eye). Numbers unchanged |
| 2026-10-14 | minor | Copy and paste in the browser: the web build never wrote to the system clipboard, so Cmd+V after Cmd+C pasted whatever text the clipboard held before (as a text layer) instead of the copied layers. Copies (layers, Copy as SVG/CSS/text/PNG, links) now go out through egui's output, which eframe writes during the copy event (`web_clipboard_tests`). Numbers unchanged. |
| 2026-10-11 | minor | Windows QA pass (gap 8): everyday workflows on Windows 11 through the desktop app and MCP; fixed: unsaved changes lost when the app was killed or crashed (recovery copies every few seconds, reopened at the next start), Ctrl+arrows/Backspace/Delete acting on the whole line in text (now by word; Ctrl+Home/End to the text's ends), scrubbed fields snapping back on release, typing on a field Tab reached, Escape not closing dialogs, skewed or vanishing linear gradients, hints cut off at the panel edge, the window-close question in English in the Spanish interface. Percentages and hours unchanged |
| 2026-10-11 | minor | Display P3 honest minimum (gap 18): the colour picker of a Display P3 file warns when a colour is outside sRGB (Outside sRGB, with what the canvas and exports do), and View ▸ Soft-proof sRGB (`view.proofSrgb`) stripes the canvas pixels sRGB can't show. Why the canvas still shows sRGB: wgpu 30 can present Display P3 or extended-range sRGB surfaces (Metal, DX12, Vulkan, WebGPU canvases), but eframe/egui-wgpu 0.36 configure an 8-bit sRGB surface with no option for the format or colour space, and egui's renderer writes sRGB-encoded UI colours; showing P3 needs those two upstream (or a patched egui-wgpu). Hardware 13–27 h → 12–26 h; gap 18 6–12 h → 5–11 h; percentages, feature breadth (~79%) and ready for real work (~47%) unchanged at this rounding; full parity ~363–697 h → ~362–696 h |
| 2026-10-11 | minor | Several windows (gap 18): File ▸ New window, a tab's Open in new window or a tab dragged off the tab strip opens files in another window with its own tabs, panels and canvas, sharing the files, preferences, libraries and clipboard; each window draws only when it needs to (idle: ~0% CPU), closing one closes its files after asking about unsaved changes (the main window too, which takes over another window instead of quitting; the last window quits), and the control channel and MCP open, list, focus and capture windows (`window.new`, `window.list`, `window.focus`, `window` on any request); the screen-reader tree covers the main window only (eframe 0.36), so with a screen reader running files stay tabs; one window in the browser. Hardware 55% → 60%, 13–27 h → 10–21 h; gap 18 6–12 h → 4–8 h (P3 on screen left); feature breadth (~79%) and ready for real work (~47%) unchanged at this rounding; full parity ~363–697 h → ~360–691 h |
| 2026-10-11 | minor | Stretch and scatter brushes and pen pressure landed (gap 9): eight brushes drawn in code (four stretched along the path, four scattered with spacing, size, rotation and scatter jitter from a seed) in the stroke settings, drawn everywhere strokes are and flattened by outline stroke; the Pencil records pen pressure as the stroke's width profile (Windows pens, browser styluses; macOS and Linux tablets report none) and can draw with a brush; Figma Draw extras breadth 45% → 65%, ready 30% → 45%, 10–18 h → 5–9 h; Strokes 2–4 h → 1–3 h; Hardware 50% → 55%, 15–30 h → 13–27 h; feature breadth (~79%) and ready for real work (~47%) unchanged at this rounding; full parity ~380–725 h → ~375–715 h |
| 2026-10-10 | major | Created from the 2026-10-10 re-measure |
