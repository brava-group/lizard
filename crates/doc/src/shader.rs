//! Portable, versioned definitions of WGSL fills stored once per document.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use lizard_color::Color;

/// The initial shader contract. Later schema versions may add effects or resource bindings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: ShaderKind,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub version: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub attribution: String,
    #[serde(default)]
    pub features: ShaderFeatures,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub source: String,
    pub entry_point: String,
    #[serde(default)]
    pub parameters: Vec<ShaderFloatParameter>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShaderKind {
    #[default]
    Fill,
    Effect,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderFeatures {
    #[serde(default)]
    pub animated: bool,
    #[serde(default)]
    pub mouse_interaction: bool,
}

/// One visible control occupies one stable `vec4` slot in `globals.params`.
/// Scalar controls use `.x`; colours use `.rgba`. The flat instance value map preserves
/// compatibility with documents created before typed controls were introduced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderFloatParameter {
    pub id: String,
    pub name: String,
    pub slot: u8,
    #[serde(default)]
    pub control: ShaderParameterControl,
    #[serde(default)]
    pub default: f32,
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub max: f32,
    #[serde(default = "default_step")]
    pub step: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ShaderSelectOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_color: Option<Color>,
    #[serde(default)]
    pub description: String,
}

fn default_step() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShaderParameterControl {
    #[default]
    Slider,
    Number,
    Select,
    Toggle,
    Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShaderSelectOption {
    pub label: String,
    pub value: f32,
}

impl ShaderFloatParameter {
    /// Default instance entries. Old scalar keys remain unchanged; colours use channel suffixes.
    pub fn default_entries(&self) -> Vec<(String, f32)> {
        if self.control == ShaderParameterControl::Color {
            let color = self.default_color.unwrap_or_default();
            return [(".r", color.r), (".g", color.g), (".b", color.b), (".a", color.a)]
                .into_iter()
                .map(|(suffix, value)| (format!("{}{}", self.id, suffix), value))
                .collect();
        }
        vec![(self.id.clone(), self.default)]
    }

    pub fn color_value(&self, values: &BTreeMap<String, f32>) -> Color {
        let fallback = self.default_color.unwrap_or_default();
        let channel = |suffix: &str, default: f32| values.get(&format!("{}.{suffix}", self.id)).copied().filter(|v| v.is_finite()).unwrap_or(default);
        Color::new(channel("r", fallback.r), channel("g", fallback.g), channel("b", fallback.b), channel("a", fallback.a))
    }

    pub fn set_color(&self, values: &mut BTreeMap<String, f32>, color: Color) {
        let color = color.sanitized();
        for (key, value) in [(".r", color.r), (".g", color.g), (".b", color.b), (".a", color.a)] {
            values.insert(format!("{}{}", self.id, key), value);
        }
    }

    /// Reject unsupported discrete values; an unbounded number keeps its full finite range.
    pub fn scalar_value(&self, value: f32) -> Option<f32> {
        if !value.is_finite() {
            return None;
        }
        match self.control {
            ShaderParameterControl::Slider => {
                (self.min.is_finite() && self.max.is_finite() && self.min <= self.max).then(|| value.clamp(self.min, self.max))
            }
            ShaderParameterControl::Number => Some(value),
            ShaderParameterControl::Select => self.options.iter().find(|option| option.value == value).map(|option| option.value),
            ShaderParameterControl::Toggle => (value == 0.0 || value == 1.0).then_some(value),
            ShaderParameterControl::Color => None,
        }
    }

    pub fn resolved_scalar(&self, values: &BTreeMap<String, f32>) -> f32 {
        values.get(&self.id).copied().and_then(|value| self.scalar_value(value)).unwrap_or(self.default)
    }
}
