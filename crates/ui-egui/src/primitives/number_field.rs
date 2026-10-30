//! A number-field primitive with content, fixed, or parent width.

use egui::{Color32, Id, Response, Ui};

use super::{ControlWidth, Menu, MenuItem};
use crate::i18n::tr;
use crate::theme::{medium, regular};
use crate::widgets::{self, BindEvent, Edit, Empty, Lead};

/// Intent returned by a number field. The containing panel owns edits and variable pickers.
#[derive(Default)]
pub struct NumberFieldResponse {
    pub edit: Option<Edit>,
    pub binding: Option<BindEvent>,
    pub sizing: Option<Response>,
    pub selected_option: Option<f64>,
}

/// Compose the existing number editor with a width policy and optional variable/sizing actions.
pub struct NumberField<'a> {
    id: Id,
    lead: Lead<'a>,
    value: Option<f64>,
    suffix: &'a str,
    step: f64,
    binding: Option<Option<&'a str>>,
    empty: Empty<'a>,
    sizing: Option<&'a str>,
    options: Option<&'a [f64]>,
}

impl<'a> NumberField<'a> {
    pub fn new(id: Id, lead: Lead<'a>, value: Option<f64>, suffix: &'a str) -> Self {
        Self { id, lead, value, suffix, step: 1.0, binding: None, empty: Empty::Text(tr("Mixed")), sizing: None, options: None }
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// `None` disables binding, `Some(None)` offers binding, and `Some(Some(name))` is bound.
    pub fn binding(mut self, binding: Option<Option<&'a str>>) -> Self {
        self.binding = binding;
        self
    }

    pub fn empty(mut self, empty: Empty<'a>) -> Self {
        self.empty = empty;
        self
    }

    /// Show a sizing selector in the trailing part of the field.
    pub fn sizing(mut self, label: &'a str) -> Self {
        self.sizing = Some(label);
        self
    }

    /// Show a joined trailing chevron with a menu of suggested numeric values.
    pub fn options(mut self, values: &'a [f64]) -> Self {
        self.options = Some(values);
        self
    }

    pub fn show(self, ui: &mut Ui, width: ControlWidth) -> NumberFieldResponse {
        let resolved = width.resolve(ui, self.intrinsic_width(ui));
        if let Some(values) = self.options {
            let (edit, binding, dropdown) =
                widgets::number_field_with_dropdown(ui, self.id, self.lead, self.value, self.suffix, resolved, self.step, self.binding, self.empty);
            let mut selected_option = None;
            Menu::popup(&dropdown).width(90.0).show(|ui| {
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for &value in values {
                        let label = widgets::fmt_num(value);
                        if MenuItem::new(&label).checked(self.value == Some(value)).show(ui).clicked() {
                            selected_option = Some(value);
                            ui.close();
                        }
                    }
                });
            });
            NumberFieldResponse { edit, binding, sizing: None, selected_option }
        } else if let Some(label) = self.sizing {
            let (edit, binding, sizing) = widgets::number_field_with_sizing(ui, self.id, self.lead, self.value, resolved, self.binding, label);
            NumberFieldResponse { edit, binding, sizing: Some(sizing), selected_option: None }
        } else {
            let (edit, binding) =
                widgets::number_field_bindable(ui, self.id, self.lead, self.value, self.suffix, resolved, self.step, self.binding, self.empty);
            NumberFieldResponse { edit, binding, sizing: None, selected_option: None }
        }
    }

    fn intrinsic_width(&self, ui: &Ui) -> f32 {
        let lead_width = match self.lead {
            Lead::None | Lead::Named(_) => 6.0,
            Lead::Text(text) => (12.0 + text.chars().count() as f32 * 7.0).max(24.0),
            Lead::Icon(..) | Lead::PositionIcon(..) | Lead::LayoutIcon(..) | Lead::AppearanceIcon(..) | Lead::PropertyIcon(..) => 24.0,
        };
        let displayed = match self.value {
            Some(value) => format!("{}{}", widgets::fmt_num(value), self.suffix),
            None => match self.empty {
                Empty::Text(text) => text.to_owned(),
                Empty::Auto => tr("Auto").to_owned(),
            },
        };
        let text = if let Some(Some(name)) = self.binding {
            if self.value.is_some() { displayed.as_str() } else { name.rsplit('/').next().unwrap_or(name) }
        } else {
            displayed.as_str()
        };
        let text_width = ui.painter().layout_no_wrap(text.to_owned(), medium(11.0), Color32::WHITE).size().x;
        let mut intrinsic = lead_width + text_width + 8.0;
        if self.binding.is_some() {
            intrinsic += 22.0;
            if self.binding.is_some_and(|binding| binding.is_some()) {
                intrinsic += 2.0;
            }
        }
        if let Some(label) = self.sizing {
            let label_width = ui.painter().layout_no_wrap(label.to_owned(), regular(11.0), Color32::WHITE).size().x;
            intrinsic = intrinsic.max(56.0 + label_width + 12.0).max(80.0);
        }
        if self.options.is_some() {
            intrinsic += 24.0;
        }
        intrinsic.max(24.0)
    }
}
