//! Contextual image actions keep the normal toolbar visible and reuse the existing crop editor.

use base64::Engine as _;
use egui::{Event, PointerButton, Rect, Shape, pos2, vec2};
use lizard_color::{ImageScaleMode, PaintKind};
use serde_json::json;

#[test]
fn selected_image_shows_a_second_toolbar_and_crop_opens_the_existing_editor() {
    let (mut harness, _) = crate::live_tests::Harness::new();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let id = harness.app.session.execute("image.place", &json!({"data": data, "x": 100, "y": 100, "width": 100, "height": 100})).unwrap()["id"]
        .as_u64()
        .unwrap();
    harness.app.session.execute("select.set", &json!({"ids": [id]})).unwrap();
    harness.frames(4);

    let (shapes, _) = harness.frame(vec![]);
    let main = crate::toolbar::pill_rect(harness.app.canvas.rect, crate::toolbar::main_width());
    let image_bar = Rect::from_min_size(
        pos2(harness.app.canvas.rect.center().x - crate::toolbar::image_toolbar_width() / 2.0, main.min.y - 48.0),
        vec2(crate::toolbar::image_toolbar_width(), 40.0),
    );
    let drawn = |target: Rect| {
        shapes.iter().any(
            |shape| matches!(shape, Shape::Rect(rect) if (rect.rect.min - target.min).length() < 0.1 && (rect.rect.max - target.max).length() < 0.1),
        )
    };
    assert!(drawn(main));
    assert!(drawn(image_bar));

    let crop = pos2(image_bar.min.x + 8.0 + 33.0, image_bar.center().y);
    let click = |pressed| Event::PointerButton { pos: crop, button: PointerButton::Primary, pressed, modifiers: Default::default() };
    harness.frame(vec![Event::PointerMoved(crop), click(true)]);
    harness.frame(vec![click(false)]);
    let fill = &harness.app.session.doc().unwrap().node(lizard_doc::NodeId(id)).unwrap().fills[0];
    assert!(matches!(&fill.kind, PaintKind::Image(image) if image.scale_mode == ImageScaleMode::Crop));
    assert!(matches!(&harness.app.ui.picker, Some(crate::colorpicker::PickerTarget::Paint { fills: true, index: 0, .. })));
}
