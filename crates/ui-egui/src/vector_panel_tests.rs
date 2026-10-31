//! The Design panel in vector edit mode keeps its width, so the canvas doesn't move under the
//! pointer when the panel's contents change.
use serde_json::json;

#[test]
fn vector_edit_options_appear_above_the_normal_toolbar_and_can_end_editing() {
    use egui::{Event, PointerButton, Rect, Shape, pos2, vec2};

    let (mut h, _) = crate::live_tests::Harness::new();
    h.app.session.execute("file.newFromTemplate", &json!({"template": "mobile-app"})).unwrap();
    let id = {
        let doc = h.app.session.doc().unwrap();
        doc.nodes().filter(|n| n.name == "Near ridge").max_by(|a, b| a.height.total_cmp(&b.height)).map(|n| n.id).unwrap()
    };
    h.app.session.execute("select.set", &json!({"ids": [id.0]})).unwrap();
    h.app.session.execute("vector.edit.begin", &json!({})).unwrap();

    h.frames(4);
    let (shapes, _) = h.frame(vec![]);
    let main = crate::toolbar::pill_rect(h.app.canvas.rect, crate::toolbar::main_width());
    let top = Rect::from_min_size(
        pos2(h.app.canvas.rect.center().x - crate::toolbar::vector_edit_toolbar_width() / 2.0, main.min.y - 48.0),
        vec2(crate::toolbar::vector_edit_toolbar_width(), 40.0),
    );
    let drawn = |target: Rect| {
        shapes.iter().any(
            |shape| matches!(shape, Shape::Rect(rect) if (rect.rect.min - target.min).length() < 0.1 && (rect.rect.max - target.max).length() < 0.1),
        )
    };
    assert!(drawn(main), "the normal bottom toolbar remains visible");
    assert!(drawn(top), "a separate vector edit surface appears above it");

    let bend = pos2(top.min.x + 8.0 + 68.0 + 70.0 + 17.0 + 68.0 + 35.0, top.center().y);
    let click = |at| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() };
    let release = |at| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() };
    let pen = pos2(main.min.x + 8.0 + 3.0 * (32.0 + 1.0 + 16.0 + 8.0) + 16.0, main.center().y);
    h.frame(vec![Event::PointerMoved(pen), click(pen)]);
    h.frame(vec![release(pen)]);
    assert_eq!(h.app.session.ds().unwrap().vector_edit.as_ref().map(|edit| edit.tool), Some(lizard_engine::vectoredit::VTool::Pen));
    assert_eq!(h.app.session.tool.tool, lizard_engine::ToolId::Move, "the main Pen button keeps vector edit mode active");
    h.frame(vec![Event::PointerMoved(bend), click(bend)]);
    h.frame(vec![release(bend)]);
    assert_eq!(h.app.session.ds().unwrap().vector_edit.as_ref().map(|edit| edit.tool), Some(lizard_engine::vectoredit::VTool::Bend));

    let close = pos2(top.max.x - 8.0 - 12.0, top.center().y);
    h.frame(vec![Event::PointerMoved(close), click(close)]);
    h.frame(vec![release(close)]);
    assert!(h.app.session.ds().unwrap().vector_edit.is_none());
}

#[test]
fn the_canvas_stays_put_entering_vector_edit_mode_and_picking_a_region() {
    let (mut h, _) = crate::live_tests::Harness::new();
    h.app.session.execute("file.newFromTemplate", &json!({"template": "mobile-app"})).unwrap();
    // A wide vector with a filled area: the panel shows its long hint until something is picked.
    let id = {
        let doc = h.app.session.doc().unwrap();
        doc.nodes().filter(|n| n.name == "Near ridge").max_by(|a, b| a.height.total_cmp(&b.height)).map(|n| n.id).unwrap()
    };
    let canvas = |h: &mut crate::live_tests::Harness| {
        h.frames(4);
        h.app.inspect()["canvas"]["rect"].clone()
    };
    h.app.session.execute("select.set", &json!({"ids": [id.0]})).unwrap();
    let before = canvas(&mut h);
    h.app.session.execute("vector.edit.begin", &json!({})).unwrap();
    assert_eq!(canvas(&mut h), before, "the hint for an empty point selection wraps instead of widening the panel");
    h.app.session.execute("vector.edit.select", &json!({"regions": [0]})).unwrap();
    assert_eq!(canvas(&mut h), before, "picking a region doesn't move the canvas either");
    h.app.session.execute("vector.edit.tool", &json!({"tool": "width"})).unwrap();
    assert_eq!(canvas(&mut h), before, "nor does the width tool's hint");
}
