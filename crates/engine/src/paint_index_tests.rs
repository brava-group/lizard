//! `fill.set` / `stroke.set` reject a negative, fractional or non-finite `index` instead of
//! clamping or truncating it onto another paint (like `fill.remove` / `stroke.remove`).

use lizard_color::{Color, Gradient, GradientKind, Paint, PaintKind};
use lizard_doc::NodeId;
use serde_json::{Value, json};

use crate::Session;
use crate::tools::{PointerEvent, PointerKind};

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn paints(s: &Session, id: NodeId) -> String {
    let n = s.doc().unwrap().node(id).unwrap();
    format!("{:?} {:?}", n.fills, n.strokes)
}

#[test]
fn paint_set_rejects_bad_index_and_leaves_the_paints_alone() {
    let mut s = Session::new();
    let r = NodeId(ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10}))["id"].as_u64().unwrap());
    for add in ["fill.add", "stroke.add"] {
        for c in ["#0000FF", "#FFFF00"] {
            ex(&mut s, add, json!({"ids": [r], "color": c}));
        }
    }
    let before = paints(&s, r);
    for cmd in ["fill.set", "stroke.set"] {
        for bad in [json!(-1), json!(0.5), json!(1.9), json!("-1"), json!("1.5"), json!("NaN"), json!("inf"), json!("abc")] {
            let res = s.execute(cmd, &json!({"ids": [r], "index": bad, "color": "#FF0000"}));
            assert!(res.is_err(), "{cmd} index {bad} was accepted: {res:?}");
            assert_eq!(paints(&s, r), before, "{cmd} index {bad} changed the document");
        }
        ex(&mut s, cmd, json!({"ids": [r], "index": 1, "color": "#FF0000"}));
        assert_ne!(paints(&s, r), before, "{cmd} with a valid index still edits");
        ex(&mut s, "edit.undo", json!({}));
        assert_eq!(paints(&s, r), before);
    }
}

#[test]
fn eyedropper_from_stroke_picker_changes_stroke_without_changing_fill() {
    let mut s = Session::new();
    let source = NodeId(ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 40, "height": 40}))["id"].as_u64().unwrap());
    ex(&mut s, "fill.set", json!({"ids": [source], "index": 0, "color": "#FF0000"}));
    let target = NodeId(ex(&mut s, "shape.rectangle", json!({"x": 100, "y": 0, "width": 40, "height": 40}))["id"].as_u64().unwrap());
    ex(&mut s, "fill.set", json!({"ids": [target], "index": 0, "color": "#00FF00"}));
    ex(&mut s, "stroke.add", json!({"ids": [target], "color": "#000000"}));
    ex(&mut s, "select.set", json!({"ids": [target]}));

    ex(&mut s, "edit.pickColor", json!({"targets": [{"id": target, "fills": false, "index": 0, "range": null}]}));
    s.pointer(&PointerEvent::at(PointerKind::Down, 20.0, 20.0, 1.0)).unwrap();

    let node = s.doc().unwrap().node(target).unwrap();
    assert_eq!(node.fills[0].solid_color().unwrap().to_hex(), "00FF00");
    assert_eq!(node.strokes[0].solid_color().unwrap().to_hex(), "FF0000");
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc().unwrap().node(target).unwrap().strokes[0].solid_color().unwrap().to_hex(), "000000");
}

#[test]
fn eyedropper_from_gradient_stop_preserves_the_gradient() {
    let mut s = Session::new();
    let source = NodeId(ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 40, "height": 40}))["id"].as_u64().unwrap());
    ex(&mut s, "fill.set", json!({"ids": [source], "index": 0, "color": "#FF0000"}));
    let target = NodeId(ex(&mut s, "shape.rectangle", json!({"x": 100, "y": 0, "width": 40, "height": 40}))["id"].as_u64().unwrap());
    let paint = Paint::gradient(Gradient::default_for(GradientKind::Linear, Color::BLACK));
    ex(&mut s, "node.set", json!({"ids": [target], "props": {"fills": [paint]}}));
    ex(&mut s, "select.set", json!({"ids": [target]}));

    ex(&mut s, "edit.pickColor", json!({"targets": [{"id": target, "fills": true, "index": 0, "range": null}], "stop": 1}));
    s.pointer(&PointerEvent::at(PointerKind::Down, 20.0, 20.0, 1.0)).unwrap();

    let PaintKind::Gradient(gradient) = &s.doc().unwrap().node(target).unwrap().fills[0].kind else { panic!("eyedropper replaced the gradient") };
    assert_eq!(gradient.stops[1].color.to_hex(), "FF0000");
}
