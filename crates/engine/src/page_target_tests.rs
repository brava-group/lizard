//! Page commands act on pages: a layer id (a rectangle, a frame) is refused before anything about
//! the layer changes, as `page.select` already does.

use figcraft_doc::{NodeId, NodeKind};
use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn id_of(v: &Value) -> NodeId {
    NodeId(v["id"].as_u64().unwrap())
}

fn name(s: &Session, id: NodeId) -> String {
    s.doc().unwrap().get(id).unwrap().name.clone()
}

#[test]
fn page_commands_reject_a_rectangle() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let r = id_of(&ex(&mut s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 10, "height": 10})));
    let before = name(&s, r);
    assert!(s.execute("page.select", &json!({"id": r})).is_err());
    assert!(s.execute("page.rename", &json!({"id": r, "name": "Not a page"})).is_err(), "rename refuses a layer");
    assert_eq!(name(&s, r), before, "the layer keeps its name");
    assert!(s.execute("page.setBackground", &json!({"id": r, "color": "#123456"})).is_err(), "background refuses a layer");
    assert!(s.execute("page.setBackgroundVisible", &json!({"id": r, "visible": false})).is_err(), "background visibility refuses a layer");
    assert!(s.execute("page.delete", &json!({"id": r})).is_err(), "delete refuses a layer");
    assert!(s.doc().unwrap().contains(r), "the layer is still there");
    assert!(s.doc().unwrap().contains(page));
}

#[test]
fn page_background_visibility_preserves_color_and_copies_with_pages() {
    let mut s = Session::new();
    let first = s.page().unwrap();
    ex(&mut s, "page.setBackground", json!({"color": "#2468AC80"}));
    let original = match s.doc().unwrap().get(first).unwrap().kind {
        NodeKind::Page { background, .. } => background,
        _ => panic!("not a page"),
    };
    ex(&mut s, "page.setBackgroundVisible", json!({"visible": false}));
    let hidden = match s.doc().unwrap().get(first).unwrap().kind {
        NodeKind::Page { background, background_visible, .. } => {
            assert!(!background_visible);
            assert_eq!(background, original);
            serde_json::to_value(&s.doc().unwrap().get(first).unwrap().kind).unwrap()
        }
        _ => panic!("not a page"),
    };
    let restored: NodeKind = serde_json::from_value(hidden).unwrap();
    assert!(matches!(restored, NodeKind::Page { background_visible: false, .. }));
    let copy = id_of(&ex(&mut s, "page.duplicate", json!({"id": first})));
    assert!(matches!(s.doc().unwrap().get(copy).unwrap().kind, NodeKind::Page { background_visible: false, .. }));
    ex(&mut s, "page.setBackgroundVisible", json!({"id": first, "visible": true}));
    assert!(
        matches!(s.doc().unwrap().get(first).unwrap().kind, NodeKind::Page { background, background_visible: true, .. } if background == original)
    );
    let shown = serde_json::to_value(&s.doc().unwrap().get(first).unwrap().kind).unwrap();
    assert!(shown.get("backgroundVisible").is_none() && shown.get("background_visible").is_none(), "visible is the backwards-compatible default");
    assert!(matches!(serde_json::from_value::<NodeKind>(shown).unwrap(), NodeKind::Page { background_visible: true, .. }));
}

#[test]
fn page_delete_keeps_a_frame_and_its_child() {
    let mut s = Session::new();
    let f = id_of(&ex(&mut s, "frame.create", json!({"x": 0, "y": 0, "width": 100, "height": 100})));
    let c = id_of(&ex(&mut s, "shape.rectangle", json!({"x": 5, "y": 5, "width": 10, "height": 10, "parent": f})));
    assert_eq!(s.doc().unwrap().get(c).unwrap().parent, Some(f));
    assert!(s.execute("page.delete", &json!({"id": f})).is_err());
    assert!(s.doc().unwrap().contains(f) && s.doc().unwrap().contains(c), "the frame and its child remain");
    assert!(s.execute("page.rename", &json!({"id": f, "name": "Not a page"})).is_err());
    assert_ne!(name(&s, f), "Not a page");
}

#[test]
fn page_commands_still_work_on_pages() {
    let mut s = Session::new();
    let first = s.page().unwrap();
    let second = id_of(&ex(&mut s, "page.add", json!({})));
    ex(&mut s, "page.rename", json!({"id": second, "name": "Flows"}));
    assert_eq!(name(&s, second), "Flows");
    ex(&mut s, "page.setBackground", json!({"id": second, "color": "#123456"}));
    match s.doc().unwrap().get(second).unwrap().kind {
        NodeKind::Page { background, .. } => assert_eq!(background.to_css_hex().to_uppercase(), "#123456"),
        _ => panic!("not a page"),
    }
    ex(&mut s, "page.delete", json!({"id": second}));
    assert!(!s.doc().unwrap().contains(second));
    assert!(s.execute("page.delete", &json!({"id": first})).is_err(), "the last ordinary page stays");
}
