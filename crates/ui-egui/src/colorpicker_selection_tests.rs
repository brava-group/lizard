//! The colour picker of a layer's paint or effect closes when that layer is no longer the
//! selection, so it never keeps editing a layer that was deselected (#396).

use egui::{Event, PointerButton};
use lizard_color::{Color, Gradient, GradientKind, Paint};
use lizard_doc::NodeId;
use lizard_engine::Session;
use serde_json::json;

use crate::colorpicker::PickerTarget;
use crate::layers_drag_tests::Ui;

fn rect(s: &mut Session, x: f64) -> NodeId {
    let v = s.execute("shape.rectangle", &json!({"x": x, "y": 0, "width": 20, "height": 20})).unwrap();
    NodeId(v["id"].as_u64().unwrap())
}

fn two() -> (Ui, NodeId, NodeId) {
    let mut s = Session::new();
    let a = rect(&mut s, 0.0);
    let b = rect(&mut s, 40.0);
    s.execute("select.set", &json!({"ids": [a]})).unwrap();
    (Ui::new(s), a, b)
}

#[test]
fn picker_closes_when_its_layer_is_deselected() {
    for effect in [false, true] {
        let (mut u, a, _) = two();
        let target = if effect {
            u.app.run("effect.add", json!({"ids": [a], "type": "dropShadow"}));
            PickerTarget::Effect { ids: vec![a], index: 0, secondary: false }
        } else {
            PickerTarget::Paint { fills: true, index: 0, ids: vec![a] }
        };
        u.app.ui.picker = Some(target.clone());
        u.frame(vec![]);
        u.frame(vec![]);
        assert_eq!(u.app.ui.picker, Some(target.clone()), "the picker opens while its layer is selected");
        u.app.run("select.none", json!({}));
        u.frame(vec![]);
        u.frame(vec![]);
        assert!(u.app.ui.picker.is_none(), "deselecting closes {target:?}");
    }
}

#[test]
fn picker_closes_when_another_layer_is_selected() {
    let (mut u, a, b) = two();
    u.app.ui.picker = Some(PickerTarget::Paint { fills: true, index: 0, ids: vec![a] });
    u.frame(vec![]);
    u.app.run("select.set", json!({"ids": [b]}));
    u.frame(vec![]);
    u.frame(vec![]);
    assert!(u.app.ui.picker.is_none(), "selecting another layer closes the picker: {:?}", u.app.ui.picker);
}

#[test]
fn gradient_picker_stays_open_while_moving_its_selected_layer_or_panning() {
    let (mut u, a, _) = two();
    let gradient = Paint::gradient(Gradient::default_for(GradientKind::Linear, Color::BLACK));
    u.app.run("node.set", json!({"ids": [a], "props": {"fills": [gradient]}}));
    let picker = PickerTarget::Paint { fills: true, index: 0, ids: vec![a] };
    u.app.ui.picker = Some(picker.clone());
    u.frame(vec![]);

    let start = u.app.to_screen(10.0, 10.0);
    let end = start + egui::vec2(12.0, 8.0);
    u.frame(vec![
        Event::PointerMoved(start),
        Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
    ]);
    u.frame(vec![Event::PointerMoved(end)]);
    u.frame(vec![Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
    assert_eq!(u.app.session.selection(), vec![a]);
    assert_eq!(u.app.ui.picker, Some(picker.clone()), "moving the same layer keeps the gradient controls open");

    let pan_start = u.app.to_screen(80.0, 80.0);
    let pan_end = pan_start + egui::vec2(25.0, 10.0);
    u.frame(vec![
        Event::PointerMoved(pan_start),
        Event::PointerButton { pos: pan_start, button: PointerButton::Middle, pressed: true, modifiers: Default::default() },
    ]);
    u.frame(vec![Event::PointerMoved(pan_end)]);
    u.frame(vec![Event::PointerButton { pos: pan_end, button: PointerButton::Middle, pressed: false, modifiers: Default::default() }]);
    assert_eq!(u.app.ui.picker, Some(picker), "middle mouse pan keeps the gradient controls open");
}
