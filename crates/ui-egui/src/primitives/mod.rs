//! Stateless UI building blocks. A primitive renders a control and reports user intent; the
//! containing panel owns document commands, picker state, and undo interactions.

mod alias_value;
mod button;
mod color_control;
mod floating_panel;
mod inline_field;
mod menu;
mod number_field;
mod property_controller;
mod property_binding;
mod tooltip;
mod search_field;
mod shortcut;
mod segmented;
mod select;
mod shimmer;
mod slider;
mod surface;
mod switch;
mod tabs;
mod text_field_with_presets;

pub use alias_value::AliasValue;
pub use button::{Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant};
pub use color_control::{ColorControl, ColorControlResponse, ColorModel, ColorValue};
pub use floating_panel::{FloatingPanel, FloatingPanelHeight, FloatingPanelWidth};
pub use inline_field::InlineField;
pub use menu::{Menu, MenuItem, MenuPopup};
pub use number_field::{NumberField, NumberFieldResponse};
pub use property_controller::{PropertyController, PropertyControllerKind, PropertyControllerResponse};
pub use property_binding::{PropertyBinding, PropertyBindingResponse};
pub use tooltip::{Tooltip, TooltipResponseExt};
pub use search_field::SearchField;
pub use shortcut::Shortcut;
pub use segmented::{Segment, SegmentContent, SegmentSize, SegmentSizing, SegmentedControl};
pub use select::{Select, SelectLead, SelectPosition, SelectVariant};
pub use shimmer::{ShimmerDirection, ShimmerText};
pub use slider::{Slider, SliderKind, SliderResponse, SliderVariant};
pub use surface::Surface;
pub use switch::Switch;
pub use tabs::{Tab, TabOverflow, Tabs};
pub use text_field_with_presets::TextFieldWithPresets;

/// Icon box shared by Select and Button menu chevrons.
pub(crate) const CONTROL_CHEVRON_SIZE: f32 = 10.0;

pub use workbench_ui::ControlWidth;
