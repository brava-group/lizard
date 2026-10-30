//! GPU rasterization and effect composition for Lizard's canvas.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod compositor;
#[cfg(not(target_arch = "wasm32"))]
pub mod export;
#[cfg(all(test, not(target_arch = "wasm32")))]
pub mod geometry_atlas;
pub mod gpu_blur;
pub mod gpu_color_profile;
pub mod gpu_diamond;
pub mod gpu_glass;
pub mod gpu_image_filter;
pub mod gpu_morph;
pub mod gpu_noise;
pub mod gpu_progressive_blur;
pub mod gpu_shader;
pub mod gpu_shadow_clip;
pub mod gpu_silhouette_shadow;
pub mod gpu_texture;
pub mod vello_scene;
