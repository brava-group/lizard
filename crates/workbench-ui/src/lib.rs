//! Reusable egui controls shared by Lizard and future native apps.
//! Host apps provide their own colors and state; this crate contains no editor engine dependency.

pub mod surface;
pub mod theme_recipe;
mod width;

pub use surface::{Surface, SurfaceTheme, SurfaceVariant};
pub use theme_recipe::{PrimaryScale, ThemeRecipe};
pub use width::ControlWidth;
