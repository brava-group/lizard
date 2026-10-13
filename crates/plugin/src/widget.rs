//! Widgets (ABI v3): a plugin draws a layer on the canvas by describing it, and FigCraft builds the
//! layers. The description comes from the plugin, so it is untrusted: [`WidgetRender::parse`]
//! checks its size, node count, depth, numbers, strings, colours and image sizes before the engine
//! sees it. The engine decides where the layers go (only inside the widget's own layer).
//!
//! ABI v4 adds to the description: lines and vectors, per-side padding and per-corner radii,
//! gradients, shadows, children that fill their auto layout frame, text styles, image fit modes,
//! editable texts (`input`), hover styles and tooltips, and the widget's property menu (`menu`).
//! A plugin written for an older API gets none of it: [`WidgetRender::parse`] takes the manifest's
//! `api` and reads the description as that version did.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::manifest::{check_id, clean};

/// Largest description a plugin may pass to `widget_render` (images included, as base64).
pub const MAX_WIDGET_BYTES: usize = 4 * 1024 * 1024;
/// Largest widget state (its JSON), and its longest key.
pub const MAX_STATE_BYTES: usize = 16 * 1024;
const MAX_STATE_KEY: usize = 128;
/// Most layers in one widget, and how deep they nest.
pub const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 16;
/// Longest text in a text layer.
pub const MAX_TEXT: usize = 1000;
const MAX_NAME: usize = 64;
/// Sizes, positions, gaps and paddings stay within this.
pub const MAX_SIZE: f64 = 4000.0;
/// Largest image (decoded bytes), and all images of one widget together.
pub const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const MAX_IMAGES_BYTES: usize = 2 * 1024 * 1024;
/// API 4. Longest SVG path data of a vector layer, and the most vectors in one widget.
pub const MAX_PATH_BYTES: usize = 8 * 1024;
pub const MAX_VECTORS: usize = 64;
/// API 4. Most colour stops in a gradient.
const MAX_STOPS: usize = 8;
/// API 4. Longest tooltip.
const MAX_TOOLTIP: usize = 200;
/// API 4. Most items in a widget's property menu, the options of one, and its longest link.
pub const MAX_MENU_ITEMS: usize = 16;
const MAX_MENU_OPTIONS: usize = 32;
const MAX_URL: usize = 2048;
const MAX_LABEL: usize = 80;
/// API 4. A widget timer's shortest and longest delay, in milliseconds.
pub const MIN_TIMER_MS: u32 = 100;
pub const MAX_TIMER_MS: u32 = 60 * 60 * 1000;
/// The first plugin API with what's marked "API 4" here.
pub(crate) const API_4: u32 = 4;

/// What a widget is made of: FigCraft's own layer kinds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetKind {
    #[default]
    Frame,
    Text,
    Rectangle,
    Ellipse,
    /// A rectangle filled with an image (PNG, JPEG…) the plugin carries in its module.
    Image,
    /// API 4. A horizontal line `width` long, drawn with `stroke`.
    Line,
    /// API 4. A shape from SVG path data (`path`); its box is the path's.
    Vector,
}

/// Auto layout direction of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetLayout {
    Horizontal,
    Vertical,
}

/// Alignment of an auto layout frame's children, along (`justify`) or across (`align`) it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetAlign {
    Min,
    Center,
    Max,
}

/// One number for every side or corner, or (API 4) four: padding as `[top, right, bottom,
/// left]`, radii as `[top-left, top-right, bottom-right, bottom-left]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Sides {
    All(f64),
    Each([f64; 4]),
}

impl Sides {
    pub fn each(self) -> [f64; 4] {
        match self {
            Sides::All(v) => [v; 4],
            Sides::Each(v) => v,
        }
    }
}

/// API 4. A gradient fill: `linear` along `angle` (degrees; 0 runs left to right, 90 top to
/// bottom) or `radial` from the centre.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetGradient {
    #[serde(rename = "type", default)]
    pub kind: GradientShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    pub stops: Vec<WidgetStop>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradientShape {
    #[default]
    Linear,
    Radial,
}

/// A gradient's colour (hex) at `at` (0–1 along it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetStop {
    pub at: f64,
    pub color: String,
}

/// API 4. A drop shadow.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetShadow {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub blur: f64,
    #[serde(default)]
    pub spread: f64,
    /// Hex; a quarter-opaque black when not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// API 4. What changes while the pointer is over a layer; FigCraft applies it itself (the plugin
/// isn't called for pointer moves).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetHover {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

/// API 4. How a text's lines sit in its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetTextAlign {
    Left,
    Center,
    Right,
}

/// API 4. How an image fills its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetFit {
    Fill,
    Fit,
    Crop,
    Tile,
}

/// One layer of a widget's description.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetNode {
    #[serde(rename = "type", default)]
    pub kind: WidgetKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Clicking this layer calls the plugin back with this name (empty: not clickable).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// Position in the parent frame (ignored inside auto layout and for the widget itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// Size; a frame with auto layout hugs its contents on an axis without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Hex colours (`#RRGGBB` or `#RRGGBBAA`); for text, `color` is the text's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(rename = "strokeWeight", default, skip_serializing_if = "Option::is_none")]
    pub stroke_weight: Option<f64>,
    /// One radius, or (API 4) four.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<Sides>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<WidgetLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    /// One padding, or (API 4) four.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<Sides>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<WidgetAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justify: Option<WidgetAlign>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Font size of a text layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// An image layer's bytes, base64.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<WidgetNode>,
    /// API 4 from here on. A gradient instead of `fill`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<WidgetGradient>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<WidgetShadow>,
    /// In an auto layout frame: take the room that's left along that axis. The frame needs a size
    /// there (its own, or because it fills too): a frame can't hug what fills it.
    #[serde(rename = "fillWidth", default, skip_serializing_if = "std::ops::Not::not")]
    pub fill_width: bool,
    #[serde(rename = "fillHeight", default, skip_serializing_if = "std::ops::Not::not")]
    pub fill_height: bool,
    /// A text's font weight (100–900), alignment and the lines it shows at most (the rest is cut
    /// with an ellipsis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(rename = "textAlign", default, skip_serializing_if = "Option::is_none")]
    pub text_align: Option<WidgetTextAlign>,
    #[serde(rename = "maxLines", default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<f64>,
    /// An image's fit (`fill` when not given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<WidgetFit>,
    /// A vector's SVG path data.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// A text the user can edit in place: committing calls the plugin back with this name and the
    /// new text (empty: not editable). At most `maxLength` characters, one line unless `multiline`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub input: String,
    #[serde(rename = "maxLength", default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<WidgetHover>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tooltip: String,
}

/// API 4. What an item of a widget's property menu is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuKind {
    /// A button.
    #[default]
    Action,
    /// On or off (`value`: a boolean).
    Toggle,
    /// One of `options` (`value`).
    Dropdown,
    /// One of the hex colours in `options` (`value`).
    Color,
    Separator,
    /// Opens `url` (http or https) when clicked; the plugin isn't called.
    Link,
}

/// API 4. One item of a widget's property menu: choosing it calls the plugin back with its `name`
/// (and its new value).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    #[serde(rename = "type", default)]
    pub kind: MenuKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

impl MenuItem {
    /// The label shown: its own, else its name.
    pub fn title(&self) -> &str {
        if self.label.is_empty() { &self.name } else { &self.label }
    }

    /// `v` as this item's new value (`None`: none given), or why it can't be: nothing for an
    /// action, a boolean for a toggle (none given: flipped), one of the options otherwise.
    pub fn coerce(&self, v: Option<&Value>) -> Result<Value, String> {
        let name = &self.name;
        match self.kind {
            MenuKind::Action => Ok(Value::Null),
            MenuKind::Toggle => match v {
                None | Some(Value::Null) => Ok(Value::Bool(!self.value.as_ref().and_then(Value::as_bool).unwrap_or(false))),
                Some(Value::Bool(b)) => Ok(Value::Bool(*b)),
                Some(_) => Err(format!("`{name}` takes true or false")),
            },
            MenuKind::Dropdown | MenuKind::Color => {
                let given = v.and_then(Value::as_str).unwrap_or_default();
                let same = |o: &&String| if self.kind == MenuKind::Color { o.eq_ignore_ascii_case(given) } else { *o == given };
                self.options
                    .iter()
                    .find(same)
                    .map(|o| Value::String(o.clone()))
                    .ok_or_else(|| format!("`{name}` must be one of: {}", self.options.join(", ")))
            }
            MenuKind::Separator | MenuKind::Link => Err("a separator or a link has no value".into()),
        }
    }

    fn check(&mut self) -> Result<(), String> {
        self.label = clean("menu label", &self.label, MAX_LABEL)?;
        if self.kind != MenuKind::Separator {
            check_id("menu item name", &self.name, MAX_NAME)?;
        }
        let name = &self.name;
        if self.options.len() > MAX_MENU_OPTIONS {
            return Err(format!("menu item `{name}` has more than {MAX_MENU_OPTIONS} options"));
        }
        let choice = matches!(self.kind, MenuKind::Dropdown | MenuKind::Color);
        if choice == self.options.is_empty() {
            return Err(format!("menu item `{name}`: `options` are for (and needed by) dropdowns and colours"));
        }
        for o in &mut self.options {
            *o = if self.kind == MenuKind::Color { hex("options", o)? } else { clean("option", o, MAX_LABEL)? };
        }
        self.value = match self.kind {
            MenuKind::Toggle => Some(Value::Bool(self.value.as_ref().and_then(Value::as_bool).unwrap_or(false))),
            // A value that isn't one of the options: the first of them.
            _ if choice => self.coerce(self.value.as_ref()).ok().or_else(|| self.options.first().cloned().map(Value::String)),
            _ => None,
        };
        if (self.kind == MenuKind::Link) == self.url.is_empty() {
            return Err(format!("menu item `{name}`: `url` is for (and needed by) links"));
        }
        let web = self.url.starts_with("https://") || self.url.starts_with("http://");
        if self.kind == MenuKind::Link && (!web || self.url.len() > MAX_URL || self.url.chars().any(|c| c.is_control() || c.is_whitespace())) {
            return Err(format!("menu item `{name}`: `url` must be an http or https address"));
        }
        Ok(())
    }
}

/// A property menu: its size, every item, names used once.
pub fn check_menu(menu: &mut [MenuItem]) -> Result<(), String> {
    if menu.len() > MAX_MENU_ITEMS {
        return Err(format!("more than {MAX_MENU_ITEMS} menu items"));
    }
    menu.iter_mut().try_for_each(MenuItem::check)?;
    let mut seen = std::collections::BTreeSet::new();
    match menu.iter().filter(|i| i.kind != MenuKind::Separator).find(|i| !seen.insert(i.name.as_str())) {
        Some(dup) => Err(format!("two menu items are called `{}`", dup.name)),
        None => Ok(()),
    }
}

/// What a widget callback hands back with `widget_render`: the widget's new state (`None`: keep
/// it) and its layers, the root a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetRender {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Map<String, Value>>,
    pub root: WidgetNode,
    /// API 4. The widget's property menu (`None`: keep it; empty: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<Vec<MenuItem>>,
}

/// What one description may still use while it's checked.
#[derive(Default)]
struct Budget {
    nodes: usize,
    image_bytes: usize,
    vectors: usize,
    /// The names of the editable texts so far (each names one text).
    inputs: Vec<String>,
}

impl WidgetRender {
    /// Parse and validate a widget description from a plugin written for plugin API `api`.
    pub fn parse(text: &str, api: u32) -> Result<WidgetRender, String> {
        if text.len() > MAX_WIDGET_BYTES {
            return Err(format!("the widget is larger than {} MB", MAX_WIDGET_BYTES / (1024 * 1024)));
        }
        let mut w: WidgetRender = serde_json::from_str(text).map_err(|e| format!("widget: {e}"))?;
        if let Some(state) = &w.state {
            check_state(state)?;
        }
        if w.root.kind != WidgetKind::Frame {
            return Err("widget: the root must be a frame".into());
        }
        if api < API_4 {
            w.menu = None;
        } else if w.root.fill_width || w.root.fill_height {
            return Err("widget: the widget itself has no frame to fill".into());
        }
        if let Some(menu) = &mut w.menu {
            check_menu(menu).map_err(|e| format!("widget: {e}"))?;
        }
        w.root.check(0, api, &mut Budget::default()).map_err(|e| format!("widget: {e}"))?;
        Ok(w)
    }
}

/// A widget's state: a JSON object of at most [`MAX_STATE_BYTES`] with short keys.
pub fn check_state(state: &Map<String, Value>) -> Result<(), String> {
    if state.keys().any(|k| k.is_empty() || k.chars().count() > MAX_STATE_KEY || k.chars().any(char::is_control)) {
        return Err(format!("widget: a state key must be 1–{MAX_STATE_KEY} characters without control characters"));
    }
    if serde_json::to_string(state).map_or(usize::MAX, |s| s.len()) > MAX_STATE_BYTES {
        return Err(format!("widget: the state is larger than {} KB", MAX_STATE_BYTES / 1024));
    }
    Ok(())
}

/// `v` when it's finite and within `lo..=hi`.
fn within(what: &str, v: Option<f64>, lo: f64, hi: f64) -> Result<(), String> {
    match v {
        Some(n) if !n.is_finite() || n < lo || n > hi => Err(format!("`{what}` must be a number from {lo} to {hi}")),
        _ => Ok(()),
    }
}

/// `s` as `#RRGGBB(AA)`.
fn hex(what: &str, s: &str) -> Result<String, String> {
    let parsed = Some(s).filter(|s| s.len() <= 9).and_then(figcraft_color::Color::from_hex);
    Ok(parsed.ok_or_else(|| format!("`{what}` must be a hex colour like #FF5A5F"))?.to_css_hex())
}

/// A colour as `#RRGGBB(AA)`.
fn colour(what: &str, c: &mut Option<String>) -> Result<(), String> {
    if let Some(s) = c {
        *s = hex(what, s)?;
    }
    Ok(())
}

impl WidgetNode {
    /// Read as plugin API 3 did: what API 4 added is refused where API 3 refused it (new kinds,
    /// four paddings or radii) and dropped where API 3 ignored it (fields it didn't know).
    fn as_api_3(&mut self) -> Result<(), String> {
        if matches!(self.kind, WidgetKind::Line | WidgetKind::Vector) {
            return Err(format!("a {} layer needs plugin API {API_4}", format!("{:?}", self.kind).to_lowercase()));
        }
        if matches!(self.padding, Some(Sides::Each(_))) || matches!(self.radius, Some(Sides::Each(_))) {
            return Err(format!("four paddings or radii need plugin API {API_4}"));
        }
        let old = std::mem::take(self);
        *self = WidgetNode {
            kind: old.kind,
            name: old.name,
            target: old.target,
            x: old.x,
            y: old.y,
            width: old.width,
            height: old.height,
            fill: old.fill,
            stroke: old.stroke,
            stroke_weight: old.stroke_weight,
            radius: old.radius,
            opacity: old.opacity,
            layout: old.layout,
            gap: old.gap,
            padding: old.padding,
            align: old.align,
            justify: old.justify,
            text: old.text,
            size: old.size,
            color: old.color,
            bold: old.bold,
            data: old.data,
            children: old.children,
            ..WidgetNode::default()
        };
        Ok(())
    }

    /// What API 4 added to a layer, checked.
    fn check_api_4(&mut self, budget: &mut Budget) -> Result<(), String> {
        let kind = self.kind;
        let only = |what: &str, used: bool, allowed: bool| {
            if used && !allowed { Err(format!("`{what}` doesn't apply to a {kind:?} layer")) } else { Ok(()) }
        };
        let text = kind == WidgetKind::Text;
        only("gradient", self.gradient.is_some(), !matches!(kind, WidgetKind::Text | WidgetKind::Image | WidgetKind::Line))?;
        only("weight", self.weight.is_some() || self.text_align.is_some() || self.max_lines.is_some(), text)?;
        only("input", !self.input.is_empty() || self.max_length.is_some() || self.multiline, text)?;
        only("fit", self.fit.is_some(), kind == WidgetKind::Image)?;
        only("path", !self.path.is_empty(), kind == WidgetKind::Vector)?;
        within("weight", self.weight, 100.0, 900.0)?;
        within("maxLines", self.max_lines, 1.0, 100.0)?;
        within("maxLength", self.max_length, 1.0, MAX_TEXT as f64)?;
        if let Some(g) = &mut self.gradient {
            if g.stops.len() < 2 || g.stops.len() > MAX_STOPS {
                return Err(format!("a gradient has 2–{MAX_STOPS} stops"));
            }
            within("angle", g.angle, -360.0, 360.0)?;
            for s in &mut g.stops {
                within("at", Some(s.at), 0.0, 1.0)?;
                s.color = hex("color", &s.color)?;
            }
        }
        if let Some(s) = &mut self.shadow {
            for (what, v) in [("x", s.x), ("y", s.y), ("spread", s.spread)] {
                within(what, Some(v), -256.0, 256.0)?;
            }
            within("blur", Some(s.blur), 0.0, 256.0)?;
            colour("color", &mut s.color)?;
        }
        if let Some(h) = &mut self.hover {
            colour("fill", &mut h.fill)?;
            colour("stroke", &mut h.stroke)?;
            within("opacity", h.opacity, 0.0, 1.0)?;
        }
        self.tooltip = clean("tooltip", &self.tooltip, MAX_TOOLTIP)?;
        // What fills an axis needs room to fill: an auto layout frame with a size on that axis,
        // given or because it fills its own frame (the widget itself has no frame to fill).
        let room = |size: Option<f64>, fills: bool| self.layout.is_some() && (size.is_some() || fills);
        let (wide, high) = (room(self.width, self.fill_width), room(self.height, self.fill_height));
        if self.children.iter().any(|c| (c.fill_width && !wide) || (c.fill_height && !high)) {
            return Err("`fillWidth` and `fillHeight` need an auto layout frame with a size on that axis (or one that fills it too)".into());
        }
        if !self.input.is_empty() {
            check_id("input", &self.input, MAX_NAME)?;
            if budget.inputs.contains(&self.input) {
                return Err(format!("two texts are the input `{}`", self.input));
            }
            budget.inputs.push(self.input.clone());
        }
        if kind == WidgetKind::Vector {
            budget.vectors += 1;
            // Path data is numbers, command letters and separators; the engine parses it.
            let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, ' ' | ',' | '.' | '-' | '+' | '\n' | '\t');
            if self.path.trim().is_empty() || self.path.len() > MAX_PATH_BYTES || !self.path.chars().all(plain) {
                return Err(format!("a vector needs `path` (SVG path data) of at most {} KB", MAX_PATH_BYTES / 1024));
            }
            if budget.vectors > MAX_VECTORS {
                return Err(format!("more than {MAX_VECTORS} vectors"));
            }
        }
        Ok(())
    }

    fn check(&mut self, depth: usize, api: u32, budget: &mut Budget) -> Result<(), String> {
        if api < API_4 {
            self.as_api_3()?;
        }
        budget.nodes += 1;
        if budget.nodes > MAX_NODES {
            return Err(format!("more than {MAX_NODES} layers"));
        }
        if depth >= MAX_DEPTH {
            return Err(format!("layers nest more than {MAX_DEPTH} deep"));
        }
        self.name = clean("name", &self.name, MAX_NAME)?;
        if !self.target.is_empty() {
            check_id("target", &self.target, MAX_NAME)?;
        }
        for (what, v) in [("x", self.x), ("y", self.y)] {
            within(what, v, -MAX_SIZE, MAX_SIZE)?;
        }
        for (what, v) in [("width", self.width), ("height", self.height), ("gap", self.gap)] {
            within(what, v, 0.0, MAX_SIZE)?;
        }
        for (what, v) in [("padding", self.padding), ("radius", self.radius)] {
            v.map_or([None; 4], |s| s.each().map(Some)).into_iter().try_for_each(|v| within(what, v, 0.0, MAX_SIZE))?;
        }
        within("strokeWeight", self.stroke_weight, 0.0, 64.0)?;
        within("opacity", self.opacity, 0.0, 1.0)?;
        within("size", self.size, 1.0, 256.0)?;
        colour("fill", &mut self.fill)?;
        colour("stroke", &mut self.stroke)?;
        colour("color", &mut self.color)?;
        let kind = self.kind;
        let only = |what: &str, used: bool, allowed: bool| {
            if used && !allowed { Err(format!("`{what}` doesn't apply to a {kind:?} layer")) } else { Ok(()) }
        };
        only("children", !self.children.is_empty(), kind == WidgetKind::Frame)?;
        only(
            "layout",
            self.layout.is_some() || self.gap.is_some() || self.padding.is_some() || self.align.is_some() || self.justify.is_some(),
            kind == WidgetKind::Frame,
        )?;
        only("text", !self.text.is_empty() || self.size.is_some() || self.color.is_some() || self.bold, kind == WidgetKind::Text)?;
        only("data", !self.data.is_empty(), kind == WidgetKind::Image)?;
        self.check_api_4(budget)?;
        if self.text.chars().count() > MAX_TEXT {
            return Err(format!("a text is longer than {MAX_TEXT} characters"));
        }
        if self.text.chars().any(|c| c.is_control() && c != '\n') {
            return Err("a text contains control characters".into());
        }
        if kind == WidgetKind::Image {
            // Base64: four characters for three bytes (checked again when decoded).
            let bytes = self.data.trim().len() / 4 * 3;
            if self.data.trim().is_empty() || bytes > MAX_IMAGE_BYTES + 3 {
                return Err(format!("an image needs `data` (base64) of at most {} KB", MAX_IMAGE_BYTES / 1024));
            }
            budget.image_bytes += bytes;
            if budget.image_bytes > MAX_IMAGES_BYTES + 3 {
                return Err(format!("the images are larger than {} MB together", MAX_IMAGES_BYTES / (1024 * 1024)));
            }
        }
        self.children.iter_mut().try_for_each(|c| c.check(depth + 1, api, budget))
    }
}
