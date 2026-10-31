//! Reusable egui controls shared by Lizard and future native apps.
//! Host apps provide their own colors and state; this crate contains no editor engine dependency.

pub mod surface;
mod width;

pub use surface::{Surface, SurfaceTheme, SurfaceVariant};
pub use width::ControlWidth;
