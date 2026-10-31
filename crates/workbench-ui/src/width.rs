//! Sizing policy shared by reusable egui controls.

/// How a control takes horizontal space from its containing layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlWidth {
    /// Occupy the remaining width offered by the parent.
    Fill,
    /// Use an explicit width in logical points.
    Fixed(f32),
    /// Size to the current value and built-in actions.
    Hug,
}

impl ControlWidth {
    pub fn resolve(self, ui: &egui::Ui, intrinsic: f32) -> f32 {
        let requested = match self {
            Self::Fill => ui.available_width(),
            Self::Fixed(width) => width,
            Self::Hug => intrinsic,
        };
        let width = if requested.is_finite() { requested } else { intrinsic };
        width.clamp(1.0, 16_384.0)
    }
}
