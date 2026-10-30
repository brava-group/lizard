//! Bounded retained tiles and GPU composition for Lizard's opt-in canvas experiment.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod compositor;
#[cfg(not(target_arch = "wasm32"))]
pub mod export;
pub mod geometry_atlas;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu_blur;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu_glass;
pub mod gpu_morph;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu_noise;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu_progressive_blur;
pub mod gpu_shadow_clip;
pub mod gpu_silhouette_shadow;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu_texture;
#[cfg(not(target_arch = "wasm32"))]
pub mod vello_scene;
