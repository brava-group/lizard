# egui-winit clipboard patch

This is the `egui-winit` 0.36.2 crate from crates.io, licensed MIT OR Apache-2.0.
FigCraft carries one change in `src/lib.rs`: forward a paste event even when the
system clipboard has no text. The desktop app then reads image data from the
clipboard itself. Without this change, `egui-winit` consumes Cmd+V on an
image-only clipboard and emits no input event.

Remove the `[patch.crates-io]` override in the workspace manifest once an
upstream release forwards the gesture.
