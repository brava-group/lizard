//! Dot grid: a panel with sliders (dots per row, their size, the gap between them), a colour and
//! a preview picture that follows them, drawn here pixel by pixel. Generate puts the grid in the
//! middle of the view as one group. The grid needs a name: without one the field says so and
//! Generate is greyed out.

use figcraft_plugin_sdk::ui::{disabled, divider, image, invalid, progress, section, slider};
use figcraft_plugin_sdk::{Event, Input, Result, Value, call, close_ui, entry, json, notify, show_ui};

/// The preview's side in pixels, and the most dots on a side.
const PREVIEW: u32 = 96;
const MAX_COLUMNS: f64 = 12.0;

/// What the panel's fields say.
#[derive(Clone, Debug, PartialEq)]
struct Grid {
    name: String,
    columns: f64,
    size: f64,
    gap: f64,
    color: String,
}

impl Grid {
    /// From the panel's values (null: the defaults).
    fn from(values: &Value) -> Grid {
        let num = |key: &str, default: f64, max: f64| values[key].as_f64().filter(|v| v.is_finite()).unwrap_or(default).clamp(0.0, max);
        Grid {
            name: values["name"].as_str().unwrap_or("Dot grid").trim().into(),
            columns: num("columns", 6.0, MAX_COLUMNS).max(2.0).round(),
            size: num("size", 16.0, 48.0).max(4.0),
            gap: num("gap", 8.0, 32.0),
            color: values["color"].as_str().unwrap_or("#0D99FF").into(),
        }
    }

    fn dots(&self) -> usize {
        (self.columns * self.columns) as usize
    }

    /// The colour as red, green and blue (black when it isn't `#RRGGBB…`).
    fn rgb(&self) -> [u8; 3] {
        let hex = self.color.trim_start_matches('#');
        [0, 2, 4].map(|i| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0))
    }

    /// Whether preview pixel (`x`, `y`) is on a dot: the grid fills the picture, the dots keep
    /// their share of each cell.
    fn on_dot(&self, x: u32, y: u32) -> bool {
        let cell = f64::from(PREVIEW) / self.columns;
        let radius = cell * self.size / (self.size + self.gap) / 2.0;
        let off = |v: u32| (f64::from(v) + 0.5) % cell - cell / 2.0;
        off(x).hypot(off(y)) <= radius
    }

    fn preview(&self) -> Vec<u8> {
        let ink = self.rgb();
        bmp(PREVIEW, |x, y| if self.on_dot(x, y) { ink } else { [245, 245, 245] })
    }

    /// Show the panel with these values.
    fn show(&self) -> Result<()> {
        let unnamed = self.name.is_empty();
        let name = json!({"type": "text", "name": "name", "label": "Name", "value": self.name});
        let generate = json!({"type": "button", "name": "generate", "label": "Generate"});
        show_ui(&json!({"title": "Dot grid", "controls": [
            if unnamed { invalid(name, "Give the grid a name") } else { name },
            slider("columns", "Dots per row", 2.0, MAX_COLUMNS, 1.0, self.columns),
            slider("size", "Dot size", 4.0, 48.0, 2.0, self.size),
            {"type": "color", "name": "color", "label": "Colour", "value": self.color},
            section("Spacing", false),
            slider("gap", "Gap", 0.0, 32.0, 1.0, self.gap),
            divider(),
            image("A preview of the dot grid", &self.preview(), 120.0),
            progress(&format!("{} of {} dots", self.dots(), (MAX_COLUMNS * MAX_COLUMNS) as usize), self.dots() as f64 / (MAX_COLUMNS * MAX_COLUMNS)),
            if unnamed { disabled(generate) } else { generate },
            {"type": "button", "name": "close", "label": "Close"}
        ]}))
    }

    /// Put the dots in the middle of the view, grouped and selected.
    fn generate(&self) -> Result<Value> {
        if self.name.is_empty() {
            return Err("Give the grid a name".into());
        }
        let step = self.size + self.gap;
        let side = self.columns * step - self.gap;
        let view = call("view.get", &json!({}))?;
        let corner = |c: &str| (view[c].as_f64().unwrap_or(0.0) - side / 2.0).round();
        let (x0, y0) = (corner("cx"), corner("cy"));
        let mut made = Vec::new();
        for i in 0..self.dots() {
            let (col, row) = ((i % self.columns as usize) as f64, (i / self.columns as usize) as f64);
            let dot = json!({"x": x0 + col * step, "y": y0 + row * step, "width": self.size, "height": self.size, "props": {"name": "Dot", "fill": self.color}});
            made.extend(call("shape.ellipse", &dot)?["id"].as_u64());
        }
        let group = call("object.group", &json!({"ids": made, "name": self.name}))?;
        call("select.set", &json!({"ids": [group["id"]]}))?;
        notify(&format!("Made {} dots", made.len()));
        Ok(json!({"dots": made.len(), "group": group["id"]}))
    }
}

/// A `size`×`size` picture as an uncompressed 24-bit BMP file, `pixel` giving red, green and blue
/// for each (x, y) from the top-left.
fn bmp(size: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let row = (size * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity((54 + row * size) as usize);
    // The file header (the pixels start at byte 54), then the 40-byte picture header.
    out.extend(b"BM");
    for v in [54 + row * size, 0, 54, 40, size, size] {
        out.extend(v.to_le_bytes());
    }
    out.extend([1, 0, 24, 0]);
    out.extend([0u8; 24]);
    // Rows run bottom to top, each padded to four bytes, blue first.
    for y in (0..size).rev() {
        for x in 0..size {
            let [r, g, b] = pixel(x, y);
            out.extend([b, g, r]);
        }
        out.extend(core::iter::repeat_n(0u8, (row - size * 3) as usize));
    }
    out
}

fn on_event(e: &Event) -> Result<Value> {
    let grid = Grid::from(&e.values);
    match e.control.as_str() {
        "generate" => grid.generate(),
        "close" => {
            close_ui();
            Ok(Value::Null)
        }
        // A field changed: the preview, the count and the name's message follow.
        _ => grid.show().map(|()| Value::Null),
    }
}

fn run(input: Input) -> Result<Value> {
    match &input.event {
        Some(e) => on_event(e),
        None => Grid::from(&Value::Null).show().map(|()| Value::Null),
    }
}

entry!(run);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_and_clamped() {
        let g = Grid::from(&Value::Null);
        assert_eq!((g.name.as_str(), g.columns, g.dots()), ("Dot grid", 6.0, 36));
        let g = Grid::from(&json!({"name": "  ", "columns": 99, "size": 1, "gap": -4, "color": "#FF8000"}));
        assert_eq!((g.name.as_str(), g.columns, g.size, g.gap, g.rgb()), ("", MAX_COLUMNS, 4.0, 0.0, [255, 128, 0]));
        assert!(g.generate().is_err(), "a grid needs a name");
        assert_eq!(Grid { color: "nope".into(), ..g }.rgb(), [0, 0, 0]);
    }

    #[test]
    fn the_preview_is_a_bmp_of_dots() {
        let g = Grid::from(&json!({"columns": 2, "size": 10, "gap": 10, "color": "#FF0000"}));
        // Two dots a side: a dot's middle is on, a cell's corner is off.
        assert!(g.on_dot(24, 24) && g.on_dot(72, 72) && !g.on_dot(0, 0) && !g.on_dot(48, 48));
        let file = g.preview();
        assert_eq!(&file[..2], b"BM");
        assert_eq!(file.len(), 54 + (PREVIEW * PREVIEW * 3) as usize);
        assert_eq!(u32::from_le_bytes([file[2], file[3], file[4], file[5]]) as usize, file.len());
        assert_eq!((u32::from_le_bytes([file[18], file[19], file[20], file[21]]), file[28]), (PREVIEW, 24));
        // The last row stored is the top one: its first pixel is background, blue first.
        let top_left = file.len() - (PREVIEW * 3) as usize;
        assert_eq!(&file[top_left..top_left + 3], &[245, 245, 245]);
        assert_eq!(bmp(1, |_, _| [1, 2, 3])[54..], [3, 2, 1, 0], "rows are padded to four bytes");
    }
}
