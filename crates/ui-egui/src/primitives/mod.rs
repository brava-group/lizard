//! Stateless UI building blocks. A primitive renders a control and reports user intent; the
//! containing panel owns document commands, picker state, and undo interactions.

mod alias_value;
mod button;
mod color_control;
mod floating_panel;
mod inline_field;
mod menu;
mod number_field;
mod search_field;
mod segmented;
mod select;
mod shimmer;
mod slider;
mod surface;
mod switch;
mod tabs;

pub use alias_value::AliasValue;
pub use button::{Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant};
pub use color_control::{ColorControl, ColorControlResponse, ColorModel, ColorValue};
pub use floating_panel::{FloatingPanel, FloatingPanelHeight, FloatingPanelWidth};
pub use inline_field::InlineField;
pub use menu::{Menu, MenuItem, MenuPopup};
pub use number_field::{NumberField, NumberFieldResponse};
pub use search_field::SearchField;
pub use segmented::{Segment, SegmentContent, SegmentSize, SegmentSizing, SegmentedControl};
pub use select::{Select, SelectLead, SelectPosition, SelectVariant};
pub use shimmer::{ShimmerDirection, ShimmerText};
pub use slider::{Slider, SliderKind, SliderResponse, SliderVariant};
pub use surface::{Surface, SurfaceVariant};
pub use switch::Switch;
pub use tabs::{Tab, TabOverflow, Tabs};

/// Icon box shared by Select and Button menu chevrons.
pub(crate) const CONTROL_CHEVRON_SIZE: f32 = 10.0;

/// How a primitive takes horizontal space from its containing layout.
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
    pub(crate) fn resolve(self, ui: &egui::Ui, intrinsic: f32) -> f32 {
        let requested = match self {
            Self::Fill => ui.available_width(),
            Self::Fixed(width) => width,
            Self::Hug => intrinsic,
        };
        let width = if requested.is_finite() { requested } else { intrinsic };
        width.clamp(1.0, 16_384.0)
    }
}
