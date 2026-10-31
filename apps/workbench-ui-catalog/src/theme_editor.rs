//! Live theme recipe editor and portable token export for the standalone catalog.

use egui::{Color32, CornerRadius, Id, Sense, Stroke, StrokeKind, Ui, vec2};
use lizard_color::{Color, Paint};
use lizard_ui_egui::primitives::{Button, ButtonVariant, ColorControl, ColorValue, ControlWidth, Segment, SegmentedControl, Slider};
use lizard_ui_egui::theme::{self, Tokens};
use lizard_ui_egui::widgets;
use serde_json::{Map, Value, json};
use workbench_ui::{PrimaryScale, ThemeRecipe};

pub fn show(
    ui: &mut Ui,
    recipe: &mut ThemeRecipe,
    picker_open: &mut bool,
    picker_anchor: &mut egui::Pos2,
    export_css: &mut bool,
    message: &mut String,
    preview_text: &mut String,
) {
    let t = Tokens::get(ui.ctx());
    ui.label("Tune the palette here, then copy or save the resolved tokens. Other catalog pages preview your changes too.");
    ui.add_space(12.0);
    egui::Frame::NONE
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Dark surface brightness").strong());
            ui.horizontal(|ui| {
                ui.label("Lighter");
                let change = Slider::new(Id::new("theme_darkness"), "Dark surface brightness")
                    .range(0.0, 100.0)
                    .step(1.0)
                    .value(Some(f64::from(recipe.darkness)))
                    .text(false)
                    .width(ControlWidth::Fixed(280.0))
                    .show(ui)
                    .change;
                if let Some((value, _)) = change {
                    recipe.darkness = value.round().clamp(0.0, 100.0) as u8;
                }
                ui.label("Darker");
                ui.label(format!("{}%", recipe.darkness));
            });
            ui.label(egui::RichText::new("50% keeps the current dark neutrals; the light theme stays unchanged.").color(t.text_secondary));
            ui.add_space(14.0);
            ui.label(egui::RichText::new("Primary color").strong());
            ui.horizontal(|ui| {
                let [red, green, blue] = recipe.primary;
                let paint = Paint::solid(Color::new(f32::from(red) / 255.0, f32::from(green) / 255.0, f32::from(blue) / 255.0, 1.0));
                let value = format!("{red:02X}{green:02X}{blue:02X}");
                let change = ColorControl::new(Id::new("theme_primary_color"), &paint, ColorValue::EditableHex(&value), 100.0).show(ui, 170.0);
                if change.open_picker {
                    *picker_open = !*picker_open;
                    if *picker_open {
                        *picker_anchor = ui.ctx().pointer_latest_pos().unwrap_or(ui.min_rect().left_top()) + vec2(8.0, 8.0);
                    }
                }
                if let Some(value) = change.edited_hex.and_then(|text| parse_hex_rgb(&text)) {
                    recipe.primary = value;
                }
                ui.label(egui::RichText::new("Hover, pressed, selected and subtle shades update together.").color(t.text_secondary));
            });
            ui.add_space(8.0);
            if Button::new("Reset to default").variant(ButtonVariant::Secondary).show(ui).clicked() {
                *recipe = ThemeRecipe::default();
            }
        });

    if *picker_open {
        let [red, green, blue] = recipe.primary;
        let color = Color::new(f32::from(red) / 255.0, f32::from(green) / 255.0, f32::from(blue) / 255.0, 1.0);
        if let Some(next) = lizard_ui_egui::colorpicker::show_standalone_solid(ui.ctx(), color, *picker_anchor, picker_open) {
            let [red, green, blue, _] = next.to_rgba8();
            recipe.primary = [red, green, blue];
        }
    }

    ui.add_space(20.0);
    ui.heading("Live preview");
    ui.add_space(8.0);
    ui.columns(2, |columns| {
        for (column, dark) in columns.iter_mut().zip([false, true]) {
            let palette = Tokens::from_recipe(dark, *recipe);
            theme::with_theme(column, palette, |ui| {
                egui::Frame::NONE
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_min_width(260.0);
                        ui.label(egui::RichText::new(if dark { "Dark" } else { "Light" }).size(16.0).strong().color(palette.text));
                        ui.label(
                            egui::RichText::new(format!("Panel {} · Field {}", hex(palette.panel), hex(palette.field))).color(palette.text_secondary),
                        );
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Primary text").color(palette.text));
                        ui.label(egui::RichText::new("Secondary text and descriptions").color(palette.text_secondary));
                        ui.add_space(10.0);
                        widgets::text_input(ui, Id::new(("theme_preview_text", dark)), preview_text, 200.0, "Editable text");
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            Button::new("Primary").variant(ButtonVariant::Primary).width(ControlWidth::Fixed(90.0)).show(ui);
                            Button::new("Secondary").variant(ButtonVariant::Secondary).width(ControlWidth::Fixed(100.0)).show(ui);
                        });
                        ui.add_space(10.0);
                        egui::Frame::NONE
                            .fill(palette.selected_row)
                            .corner_radius(CornerRadius::same(5))
                            .inner_margin(egui::Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("Selected item").color(palette.text));
                            });
                    });
            });
        }
    });

    ui.add_space(20.0);
    ui.heading("Primary shades");
    ui.add_space(8.0);
    let shades = resolved_shades(*recipe, true);
    ui.horizontal_wrapped(|ui| {
        for (name, color) in [
            ("Base", shades.base),
            ("Hover", shades.hover),
            ("Pressed", shades.pressed),
            ("Secondary", shades.secondary),
            ("Subtle", shades.subtle),
            ("Selected", shades.selected),
            ("Text", shades.text),
        ] {
            swatch(ui, name, color);
        }
    });

    ui.add_space(20.0);
    ui.heading("Export resolved values");
    ui.label("Both light and dark token sets are included. The recipe can be reused by another Workbench UI host.");
    ui.add_space(8.0);
    let formats = [Segment::text("JSON"), Segment::text("CSS")];
    if let Some(index) = SegmentedControl::new(&formats, Some(usize::from(*export_css))).show(ui) {
        *export_css = index == 1;
    }
    let mut output = if *export_css { css(*recipe) } else { json_export(*recipe) };
    ui.add(egui::TextEdit::multiline(&mut output).code_editor().desired_width(ui.available_width().min(960.0)).desired_rows(14).interactive(false));
    ui.horizontal(|ui| {
        if Button::new(if *export_css { "Copy CSS" } else { "Copy JSON" }).variant(ButtonVariant::Secondary).show(ui).clicked() {
            ui.ctx().copy_text(output.clone());
            *message = "Copied to clipboard".into();
        }
        if Button::new(if *export_css { "Save CSS…" } else { "Save JSON…" }).variant(ButtonVariant::Secondary).show(ui).clicked() {
            let (name, extension) = if *export_css { ("workbench-theme.css", "css") } else { ("workbench-theme.json", "json") };
            if let Some(path) = rfd::FileDialog::new().add_filter(extension.to_uppercase(), &[extension]).set_file_name(name).save_file() {
                *message = match std::fs::write(&path, output.as_bytes()) {
                    Ok(()) => format!("Saved {}", path.display()),
                    Err(error) => format!("Could not save: {error}"),
                };
            }
        }
        ui.label(egui::RichText::new(message.as_str()).color(t.text_secondary));
    });
}

fn parse_hex_rgb(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        return None;
    }
    let number = u32::from_str_radix(value, 16).ok()?;
    Some([((number >> 16) & 0xff) as u8, ((number >> 8) & 0xff) as u8, (number & 0xff) as u8])
}

fn swatch(ui: &mut Ui, name: &str, color: Color32) {
    ui.vertical(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(94.0, 32.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(5), color);
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, Tokens::get(ui.ctx()).border), StrokeKind::Inside);
        ui.label(name);
        ui.monospace(hex(color));
    });
}

fn hex(color: Color32) -> String {
    let [r, g, b, a] = color.to_array();
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

fn token_map(tokens: Tokens) -> Map<String, Value> {
    tokens.named_colors().into_iter().map(|(name, color)| (name.to_owned(), json!(hex(color)))).collect()
}

fn resolved_shades(recipe: ThemeRecipe, dark: bool) -> PrimaryScale {
    let tokens = Tokens::from_recipe(dark, recipe);
    let mut shades = recipe.primary_scale(dark, tokens.panel);
    shades.base = tokens.accent;
    shades.hover = tokens.accent_hover;
    shades.pressed = tokens.accent_pressed;
    shades.text = tokens.accent_text;
    shades.selected = tokens.selected_row;
    shades.selected_child = tokens.selected_row_child;
    shades.on_primary = tokens.accent_on;
    shades
}

fn shade_map(recipe: ThemeRecipe, dark: bool) -> Value {
    let shades = resolved_shades(recipe, dark);
    json!({
        "base": hex(shades.base), "hover": hex(shades.hover), "pressed": hex(shades.pressed),
        "secondary": hex(shades.secondary), "subtle": hex(shades.subtle), "text": hex(shades.text),
        "selected": hex(shades.selected), "selectedHover": hex(shades.selected_hover),
        "selectedChild": hex(shades.selected_child), "onPrimary": hex(shades.on_primary)
    })
}

fn json_export(recipe: ThemeRecipe) -> String {
    let document = json!({
        "schema": "workbench-ui/theme-v1",
        "recipe": {"darkness": recipe.darkness, "primary": hex(Color32::from_rgb(recipe.primary[0], recipe.primary[1], recipe.primary[2]))},
        "light": {"tokens": token_map(Tokens::from_recipe(false, recipe)), "primaryShades": shade_map(recipe, false)},
        "dark": {"tokens": token_map(Tokens::from_recipe(true, recipe)), "primaryShades": shade_map(recipe, true)}
    });
    match serde_json::to_string_pretty(&document) {
        Ok(text) => text,
        Err(error) => format!("Could not serialize theme: {error}"),
    }
}

fn css(recipe: ThemeRecipe) -> String {
    let mut out = format!(
        "/* Workbench UI theme · darkness {} · primary {} */\n",
        recipe.darkness,
        hex(Color32::from_rgb(recipe.primary[0], recipe.primary[1], recipe.primary[2]))
    );
    for (selector, dark) in [(":root, [data-theme=\"light\"]", false), ("[data-theme=\"dark\"]", true)] {
        out.push_str(selector);
        out.push_str(" {\n");
        for (name, color) in Tokens::from_recipe(dark, recipe).named_colors() {
            out.push_str(&format!("  --workbench-{name}: {};\n", hex(color)));
        }
        let shades = resolved_shades(recipe, dark);
        for (name, color) in [
            ("primary", shades.base),
            ("primary-hover", shades.hover),
            ("primary-pressed", shades.pressed),
            ("primary-secondary", shades.secondary),
            ("primary-subtle", shades.subtle),
            ("primary-selected", shades.selected),
            ("primary-selected-hover", shades.selected_hover),
            ("primary-selected-child", shades.selected_child),
            ("primary-text", shades.text),
            ("primary-on", shades.on_primary),
        ] {
            out.push_str(&format!("  --workbench-{name}: {};\n", hex(color)));
        }
        out.push_str("}\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_primary_matches_the_preview_tokens() {
        let recipe = ThemeRecipe { darkness: 80, primary: [0xc5, 0x68, 0xff] };
        let expected = hex(Tokens::from_recipe(true, recipe).accent);
        let document = serde_json::from_str::<Value>(&json_export(recipe)).ok();
        assert_eq!(document.as_ref().and_then(|value| value.pointer("/dark/tokens/accent")).and_then(Value::as_str), Some(expected.as_str()));
        assert_eq!(document.as_ref().and_then(|value| value.pointer("/dark/primaryShades/base")).and_then(Value::as_str), Some(expected.as_str()));
        assert!(css(recipe).contains(&format!("--workbench-accent: {expected};")));
    }

    #[test]
    fn hex_input_accepts_rgb_and_rejects_incomplete_values() {
        assert_eq!(parse_hex_rgb("#C568FF"), Some([0xc5, 0x68, 0xff]));
        assert_eq!(parse_hex_rgb("C56"), None);
    }
}
