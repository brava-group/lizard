use figcraft_color::{Color, ColorProfile, Paint};
use figcraft_doc::Document;

use super::import_svg_with_notes;

fn imported_fill(value: &str) -> Result<Color, Box<dyn std::error::Error>> {
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="10" height="10" fill="{value}"/></svg>"#);
    let mut doc = Document::new("color");
    doc.color_profile = ColorProfile::Srgb;
    let page = doc.first_page().ok_or("missing page")?;
    let imported = import_svg_with_notes(&mut doc, svg.as_bytes(), page, 0.0, 0.0, "svg").map_err(|e| e.to_string())?;
    let id = *doc.node(imported.frame).ok_or("missing frame")?.children.first().ok_or("missing shape")?;
    doc.node(id).and_then(|node| node.fills.first()).and_then(Paint::solid_color).ok_or_else(|| "missing solid fill".into())
}

#[test]
fn css_color_functions_keep_svg_fills() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(imported_fill("color(srgb 1 0 0)")?.to_rgba8(), [255, 0, 0, 255]);
    assert_eq!(imported_fill("color-mix(in srgb, red, blue)")?.to_rgba8(), [128, 0, 128, 255]);
    assert_eq!(imported_fill("color-mix(in oklab, white, black)")?.to_rgba8(), [99, 99, 99, 255]);
    let p3 = Color::new(0.2, 0.6, 0.3, 1.0);
    let expected = ColorProfile::DisplayP3.conversion_to(ColorProfile::Srgb).apply(p3);
    assert_eq!(imported_fill("color(display-p3 0.2 0.6 0.3)")?.to_rgba8(), expected.to_rgba8());
    let outer = imported_fill("color-mix(in srgb, color(srgb 1 0 0), color(srgb 0 0 1))")?;
    assert_eq!(outer.to_rgba8(), [128, 0, 128, 255]);
    Ok(())
}
