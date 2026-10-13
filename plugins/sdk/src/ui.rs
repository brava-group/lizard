//! Builders for what a plugin describes to FigCraft: a widget's layers ([`Node`]) and property
//! menu ([`menu_action`] and its siblings), and the controls of a panel that plugin API 4 added
//! ([`slider`], [`image`], [`progress`], [`divider`], [`section`]). They only assemble the JSON
//! that `docs/plugins.md` documents; [`Node::set`] reaches anything they don't name, and plain
//! `json!` works everywhere too.
//!
//! ```ignore
//! use figcraft_plugin_sdk::ui::{Node, menu_toggle, render};
//!
//! let root = Node::column()
//!     .padding(12.0)
//!     .gap(8.0)
//!     .radius(12.0)
//!     .fill("#FFFFFF")
//!     .shadow(0.0, 2.0, 8.0, "#00000033")
//!     .child(Node::text("Title").input("title", 60).weight(600.0).tooltip("Click to rename"))
//!     .child(Node::line(160.0).stroke("#E6E6E6", 1.0).fill_width());
//! render(&json!({"title": "Title"}), root, Some(vec![menu_toggle("done", "Done", false)]))?;
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

/// One layer of a widget's description.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node(Map<String, Value>);

impl From<Node> for Value {
    fn from(n: Node) -> Value {
        Value::Object(n.0)
    }
}

/// Setters that put one value under one key.
macro_rules! setters {
    ($($(#[$doc:meta])* $name:ident($ty:ty) => $key:literal;)*) => {
        $($(#[$doc])* pub fn $name(self, v: $ty) -> Node { self.set($key, v) })*
    };
}

impl Node {
    fn of(kind: &str) -> Node {
        Node::default().set("type", kind)
    }

    /// A frame without auto layout: its children sit at their `x` and `y`.
    pub fn frame() -> Node {
        Node::of("frame")
    }

    /// An auto layout frame with its children side by side; it hugs them on an axis without a size.
    pub fn row() -> Node {
        Node::of("frame").set("layout", "horizontal")
    }

    /// An auto layout frame with its children one under the other.
    pub fn column() -> Node {
        Node::of("frame").set("layout", "vertical")
    }

    /// A text. With a `width` it wraps; without one it grows with its text.
    pub fn text(text: &str) -> Node {
        Node::of("text").set("text", text)
    }

    pub fn rectangle(width: f64, height: f64) -> Node {
        Node::of("rectangle").width(width).height(height)
    }

    pub fn ellipse(width: f64, height: f64) -> Node {
        Node::of("ellipse").width(width).height(height)
    }

    /// API 4. A horizontal line (black and 1 thick unless it gets a [`stroke`](Node::stroke)).
    pub fn line(length: f64) -> Node {
        Node::of("line").width(length)
    }

    /// API 4. A shape from SVG path data (`M0 0 L10 0 L5 8 Z`); with a size it's scaled to it.
    pub fn vector(path: &str) -> Node {
        Node::of("vector").set("path", path)
    }

    /// An image from its file's bytes (PNG, JPEG, BMP…), usually `include_bytes!` in the plugin.
    pub fn image(bytes: &[u8]) -> Node {
        Node::of("image").set("data", base64(bytes))
    }

    /// Any property by its name in the widget description.
    pub fn set(mut self, key: &str, value: impl Into<Value>) -> Node {
        self.0.insert(key.into(), value.into());
        self
    }

    setters! {
        /// The layer's name in the layers panel.
        name(&str) => "name";
        /// Clicking the layer calls the plugin back with this name.
        target(&str) => "target";
        x(f64) => "x";
        y(f64) => "y";
        width(f64) => "width";
        height(f64) => "height";
        /// A hex colour (`#RRGGBB` or `#RRGGBBAA`).
        fill(&str) => "fill";
        opacity(f64) => "opacity";
        /// The same radius on every corner.
        radius(f64) => "radius";
        /// Auto layout: the space between children.
        gap(f64) => "gap";
        /// Auto layout: the same padding on every side.
        padding(f64) => "padding";
        /// Auto layout: where the children sit across it: `min`, `center` or `max`.
        align(&str) => "align";
        /// Auto layout: where the children sit along it: `min`, `center` or `max`.
        justify(&str) => "justify";
        /// A text's font size.
        font_size(f64) => "size";
        /// A text's colour (hex).
        color(&str) => "color";
        /// API 4. A text's font weight, 100–900.
        weight(f64) => "weight";
        /// API 4. A text's alignment in its width: `left`, `center` or `right`.
        text_align(&str) => "textAlign";
        /// API 4. A text with a width shows this many lines at most; the rest is cut with an ellipsis.
        max_lines(f64) => "maxLines";
        /// API 4. An image's fit in its box: `fill`, `fit`, `crop` or `tile`.
        fit(&str) => "fit";
        /// API 4. Shown while the pointer rests on the layer.
        tooltip(&str) => "tooltip";
    }

    pub fn size(self, width: f64, height: f64) -> Node {
        self.width(width).height(height)
    }

    pub fn stroke(self, color: &str, weight: f64) -> Node {
        self.set("stroke", color).set("strokeWeight", weight)
    }

    pub fn bold(self) -> Node {
        self.set("bold", true)
    }

    /// API 4. Radii for the top-left, top-right, bottom-right and bottom-left corners.
    pub fn radii(self, corners: [f64; 4]) -> Node {
        self.set("radius", json!(corners))
    }

    /// API 4. Paddings for the top, right, bottom and left sides.
    pub fn paddings(self, sides: [f64; 4]) -> Node {
        self.set("padding", json!(sides))
    }

    /// API 4. In an auto layout frame: take the width that's left.
    pub fn fill_width(self) -> Node {
        self.set("fillWidth", true)
    }

    /// API 4. In an auto layout frame: take the height that's left.
    pub fn fill_height(self) -> Node {
        self.set("fillHeight", true)
    }

    fn gradient(self, kind: &str, angle: Option<f64>, stops: &[(f64, &str)]) -> Node {
        let stops: Vec<Value> = stops.iter().map(|(at, color)| json!({"at": at, "color": color})).collect();
        self.set("gradient", json!({"type": kind, "angle": angle, "stops": stops}))
    }

    /// API 4. A linear gradient instead of a fill: `angle` in degrees (0 runs left to right, 90
    /// top to bottom) and two to eight `(position 0–1, hex colour)` stops.
    pub fn linear_gradient(self, angle: f64, stops: &[(f64, &str)]) -> Node {
        self.gradient("linear", Some(angle), stops)
    }

    /// API 4. A radial gradient from the centre instead of a fill.
    pub fn radial_gradient(self, stops: &[(f64, &str)]) -> Node {
        self.gradient("radial", None, stops)
    }

    /// API 4. A drop shadow.
    pub fn shadow(self, x: f64, y: f64, blur: f64, color: &str) -> Node {
        self.set("shadow", json!({"x": x, "y": y, "blur": blur, "color": color}))
    }

    /// API 4. A text the user can edit in place (a click starts it; Enter or clicking elsewhere
    /// commits): the plugin is then called with [`WidgetEvent::Text`](crate::WidgetEvent::Text)
    /// carrying `name` and the text, at most `max_length` characters, on one line.
    pub fn input(self, name: &str, max_length: u32) -> Node {
        self.set("input", name).set("maxLength", max_length)
    }

    /// API 4. An [`input`](Node::input) that takes several lines (Enter starts a new one;
    /// clicking elsewhere or Escape commits).
    pub fn multiline(self) -> Node {
        self.set("multiline", true)
    }

    /// API 4. What changes while the pointer is over the layer: its fill, its stroke and its
    /// opacity (`None`: as it is). FigCraft shows it; the plugin isn't called.
    pub fn hover(self, fill: Option<&str>, stroke: Option<&str>, opacity: Option<f64>) -> Node {
        self.set("hover", json!({"fill": fill, "stroke": stroke, "opacity": opacity}))
    }

    /// Add a layer inside this frame.
    pub fn child(mut self, child: Node) -> Node {
        match self.0.entry("children").or_insert_with(|| Value::Array(Vec::new())) {
            Value::Array(a) => a.push(child.into()),
            other => *other = json!([Value::from(child)]),
        }
        self
    }

    /// Add layers inside this frame.
    pub fn children(self, children: impl IntoIterator<Item = Node>) -> Node {
        children.into_iter().fold(self, Node::child)
    }
}

/// Draw the widget being called: its new `state` (a JSON object), its layers and, with `Some`,
/// its property menu (`None` keeps the menu it has; an empty one removes it).
pub fn render(state: &Value, root: Node, menu: Option<Vec<Value>>) -> crate::Result<()> {
    let mut w = json!({"state": state, "root": Value::from(root)});
    if let Some(menu) = menu {
        w["menu"] = Value::Array(menu);
    }
    crate::render_widget(&w)
}

/// API 4. A button in the widget's property menu: choosing it calls the plugin with
/// [`WidgetEvent::Menu`](crate::WidgetEvent::Menu) carrying `name`.
pub fn menu_action(name: &str, label: &str) -> Value {
    json!({"type": "action", "name": name, "label": label})
}

/// API 4. An on/off item; the plugin gets the new state as the value.
pub fn menu_toggle(name: &str, label: &str, on: bool) -> Value {
    json!({"type": "toggle", "name": name, "label": label, "value": on})
}

/// API 4. A menu of `options` showing `value`; the plugin gets the option picked.
pub fn menu_dropdown(name: &str, label: &str, options: &[&str], value: &str) -> Value {
    json!({"type": "dropdown", "name": name, "label": label, "options": options, "value": value})
}

/// API 4. A row of hex colours to pick one from, `value` marked; the plugin gets the one picked.
pub fn menu_color(name: &str, label: &str, palette: &[&str], value: &str) -> Value {
    json!({"type": "color", "name": name, "label": label, "options": palette, "value": value})
}

/// API 4. A line between menu items.
pub fn menu_separator() -> Value {
    json!({"type": "separator"})
}

/// API 4. A link (http or https) FigCraft opens when it's clicked; the plugin isn't called.
pub fn menu_link(name: &str, label: &str, url: &str) -> Value {
    json!({"type": "link", "name": name, "label": label, "url": url})
}

/// API 4, panels. A slider from `min` to `max` in `step`s. While it's dragged the plugin is
/// called back a few times a second, and once more when it's let go.
pub fn slider(name: &str, label: &str, min: f64, max: f64, step: f64, value: f64) -> Value {
    json!({"type": "slider", "name": name, "label": label, "min": min, "max": max, "step": step, "value": value})
}

/// API 4, panels. A picture from its file's bytes, `height` high (16–400), fitted in the panel's
/// width; `label` names it for screen readers.
pub fn image(label: &str, bytes: &[u8], height: f64) -> Value {
    json!({"type": "image", "label": label, "data": base64(bytes), "height": height, "fit": "fit"})
}

/// API 4, panels. A progress bar filled to `fraction` (0–1).
pub fn progress(label: &str, fraction: f64) -> Value {
    json!({"type": "progress", "label": label, "value": fraction})
}

/// API 4, panels. A line between controls.
pub fn divider() -> Value {
    json!({"type": "divider"})
}

/// API 4, panels. A heading that folds the controls after it, up to the next section or divider.
pub fn section(label: &str, open: bool) -> Value {
    json!({"type": "section", "label": label, "value": open})
}

/// API 4, panels. `control` greyed out: it can't be used.
pub fn disabled(mut control: Value) -> Value {
    control["disabled"] = Value::Bool(true);
    control
}

/// API 4, panels. `control` with a validation message under it.
pub fn invalid(mut control: Value, message: &str) -> Value {
    control["error"] = Value::String(message.into());
    control
}

/// `bytes` as base64, the way image data travels in descriptions.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            let sextet = ALPHABET.get((n >> (18 - 6 * i)) as usize & 63).copied().unwrap_or(b'A');
            out.push(if i <= chunk.len() { char::from(sextet) } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_make_the_documented_json() {
        let n = Node::row().gap(4.0).paddings([1.0, 2.0, 3.0, 4.0]).child(Node::text("Hi").input("title", 20).multiline()).child(Node::line(10.0));
        let v = Value::from(n);
        assert_eq!(
            (v["type"].as_str(), v["layout"].as_str(), v["padding"].clone()),
            (Some("frame"), Some("horizontal"), json!([1.0, 2.0, 3.0, 4.0]))
        );
        assert_eq!(v["children"][0], json!({"type": "text", "text": "Hi", "input": "title", "maxLength": 20, "multiline": true}));
        assert_eq!(v["children"][1], json!({"type": "line", "width": 10.0}));
        let g = Value::from(Node::rectangle(8.0, 8.0).linear_gradient(90.0, &[(0.0, "#000000"), (1.0, "#FFFFFF")]).hover(
            Some("#FF0000"),
            None,
            Some(0.5),
        ));
        assert_eq!(g["gradient"]["stops"][1], json!({"at": 1.0, "color": "#FFFFFF"}));
        assert_eq!(g["hover"], json!({"fill": "#FF0000", "stroke": null, "opacity": 0.5}));
        assert_eq!(menu_color("tint", "Tint", &["#FF0000"], "#FF0000")["options"], json!(["#FF0000"]));
        assert_eq!(disabled(invalid(slider("n", "N", 0.0, 10.0, 1.0, 3.0), "Too low"))["error"], "Too low");
        assert!(render(&json!({}), Node::frame(), Some(Vec::new())).is_err(), "no host off WebAssembly");
    }

    #[test]
    fn base64_pads() {
        for (bytes, text) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64(bytes.as_bytes()), text);
        }
    }
}
