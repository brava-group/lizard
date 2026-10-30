pub use figcraft_canvas_gpu::{compositor, gpu_blur, vello_scene as gpu_scene};
// The retired atlas stays available to legacy benchmark binaries without exposing it from
// the production canvas GPU crate.
#[path = "../../../crates/canvas-gpu/src/geometry_atlas.rs"]
pub mod geometry_atlas;
pub mod gpu_live_residual;
pub mod gpu_residual;
pub mod residual;
