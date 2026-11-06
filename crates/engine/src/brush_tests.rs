//! Brushes and pen pressure through the session: `stroke.brush`, rendering and outlining brush
//! strokes, and the Pencil recording pressure (and its brush) into the stroke it draws.

use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;
use crate::tools::{PointerEvent, PointerKind, pressure_profile};
use lizard_doc::{NodeId, NodeKind};
use lizard_geom::Point;

fn id(v: Value) -> NodeId {
    id_of(&v["id"]).unwrap()
}

/// A 200-long horizontal vector with a 10 px black stroke.
fn line(s: &mut Session) -> NodeId {
    let v = id(s.execute("vector.create", &json!({"path": "M0 0 L200 0", "x": 0, "y": 0})).unwrap());
    s.execute("node.set", &json!({"id": v, "props": {"strokes": ["#000000"], "strokeWeight": 10, "strokeAlign": "center"}})).unwrap();
    v
}

/// The alpha of the page's layers at page point (`x`, `y`), rendered at 1x without the page colour.
fn alpha(s: &Session, x: f64, y: f64) -> u8 {
    let doc = s.doc().unwrap();
    let view = lizard_render::View { origin: (x - 1.0, y - 1.0), scale: 1.0, width: 3, height: 3 };
    let mut pix = lizard_render::Pixmap::new(3, 3).unwrap();
    lizard_render::render_nodes_into(doc, &doc.node(s.page().unwrap()).unwrap().children, &view, &mut pix);
    pix.data()[(3 + 1) * 4 + 3]
}

#[test]
fn brushes_draw_along_the_stroke_and_outline_to_vectors() {
    let mut s = Session::new();
    let v = line(&mut s);
    assert_eq!(alpha(&s, 15.0, 0.0), 255, "a plain stroke covers the line");
    assert!(s.execute("stroke.brush", &json!({"ids": [v], "preset": "glitter"})).is_err());
    // Dots every 2 weights: centres at 10, 30, 50 … with gaps between.
    s.execute("stroke.brush", &json!({"ids": [v], "preset": "dots", "spacing": 2})).unwrap();
    assert_eq!(alpha(&s, 10.0, 0.0), 255);
    assert_eq!(alpha(&s, 20.0, 0.0), 0, "the gap between two dots");
    let b = s.execute("node.get", &json!({"id": v})).unwrap()["brush"].clone();
    assert_eq!((b["preset"].as_str(), b["spacing"].as_f64()), (Some("dots"), Some(2.0)), "{b}");
    // A stretch brush tapers to a point at both ends.
    s.execute("stroke.brush", &json!({"ids": [v], "preset": "taper"})).unwrap();
    assert_eq!(alpha(&s, 100.0, 2.0), 255);
    assert_eq!(alpha(&s, 2.0, 3.0), 0);
    // Outline stroke turns the brush into a filled vector.
    s.execute("vector.outlineStroke", &json!({"ids": [v]})).unwrap();
    let doc = s.doc().unwrap();
    let outlined = doc.nodes().find(|n| matches!(n.kind, NodeKind::Vector { .. }) && !n.fills.is_empty()).expect("an outlined vector");
    assert!(outlined.strokes.is_empty() || outlined.stroke.brush.is_none());
    assert_eq!(alpha(&s, 100.0, 2.0), 255, "looks the same after outlining");
    // Removing the brush: a plain stroke again.
    let mut s = Session::new();
    let v = line(&mut s);
    s.execute("stroke.brush", &json!({"ids": [v], "preset": "dots", "spacing": 2})).unwrap();
    s.execute("stroke.brush", &json!({"ids": [v], "preset": null})).unwrap();
    assert_eq!(alpha(&s, 20.0, 0.0), 255);
    // Exports draw it too.
    s.execute("stroke.brush", &json!({"ids": [v], "preset": "stars"})).unwrap();
    let doc = s.doc().unwrap();
    assert!(lizard_svg::export_svg(doc, &[v], &lizard_svg::SvgOptions::default()).unwrap().contains("<path"));
    assert!(lizard_pdf::export_nodes_pdf(doc, &[v]).is_ok());
}

#[test]
fn pressure_becomes_a_width_profile() {
    let pts: Vec<Point> = (0..=10).map(|i| Point::new(f64::from(i) * 10.0, 0.0)).collect();
    assert!(pressure_profile(&pts, &[None; 11]).is_none(), "a mouse: a plain stroke");
    let light_to_hard: Vec<Option<f32>> = (0..=10).map(|i| Some(i as f32 / 10.0)).collect();
    let prof = pressure_profile(&pts, &light_to_hard).unwrap();
    assert_eq!(prof.len(), 11);
    assert_eq!(prof.first(), Some(&[0.0, 0.1]), "no pressure still draws a hairline");
    assert_eq!(prof.get(5), Some(&[0.5, 1.0]), "medium pressure: the stroke weight");
    assert_eq!(prof.last(), Some(&[1.0, 2.0]));
    assert!(pressure_profile(&[Point::ZERO, Point::ZERO], &[Some(0.5), Some(0.5)]).is_none(), "no length");
}

#[test]
fn brush_preview_uses_selected_geometry_during_the_drag() {
    let mut s = Session::new();
    s.execute("tool.set", &json!({"tool": "brush"})).unwrap();
    s.execute("tool.drawStyle", &json!({"tool": "brush", "preset": "heist", "weight": 20})).unwrap();
    let event = |kind, x: f64, p: f32| PointerEvent { pressure: Some(p), ..PointerEvent::at(kind, x, 40.0, 1.0) };
    s.pointer(&event(PointerKind::Down, 0.0, 0.2)).unwrap();
    assert!(s.live_brush_outline().is_none(), "a click without a path makes no layer");
    s.pointer(&event(PointerKind::Move, 40.0, 0.2)).unwrap();
    let first = s.live_brush_outline().expect("live filled brush shape");
    s.pointer(&event(PointerKind::Move, 120.0, 0.8)).unwrap();
    let last = s.live_brush_outline().expect("updated brush shape");
    assert_ne!(first, last, "the preview grows during the drag");
    s.pointer(&event(PointerKind::Up, 120.0, 0.8)).unwrap();
    assert!(s.live_brush_outline().is_none(), "the committed layer replaces the preview");
    let node = s.doc().unwrap().nodes().find(|n| matches!(n.kind, NodeKind::Vector { .. })).expect("committed brush layer");
    assert_eq!(node.stroke.brush.as_ref().map(|b| b.preset.as_str()), Some("heist"));
    assert!(node.stroke.profile().is_some(), "pen pressure is retained");
}

#[test]
fn the_pencil_records_pressure_and_its_brush() {
    let mut s = Session::new();
    s.execute("tool.set", &json!({"tool": "pencil"})).unwrap();
    assert!(s.execute("tool.pencilBrush", &json!({"preset": "nope"})).is_err());
    s.execute("tool.pencilBrush", &json!({"preset": "taper"})).unwrap();
    let ev = |kind, x: f64, p: f32| PointerEvent { pressure: Some(p), ..PointerEvent::at(kind, x, 50.0 + (x / 20.0).sin() * 10.0, 1.0) };
    s.pointer(&ev(PointerKind::Down, 0.0, 0.2)).unwrap();
    for i in 1..=40 {
        s.pointer(&ev(PointerKind::Move, f64::from(i) * 5.0, 0.2 + 0.02 * i as f32)).unwrap();
    }
    s.pointer(&ev(PointerKind::Up, 200.0, 1.0)).unwrap();
    let doc = s.doc().unwrap();
    let n = doc.nodes().find(|n| matches!(n.kind, NodeKind::Vector { .. })).expect("a pencil stroke");
    let prof = n.stroke.profile().expect("pressure recorded");
    assert!(prof.scale_at(0.0) < 0.5 && prof.scale_at(1.0) > 1.9, "light to hard: {prof:?}");
    assert_eq!(n.stroke.brush.as_ref().map(|b| b.preset.as_str()), Some("taper"));
    // A mouse (no pressure) draws a plain uniform stroke.
    s.execute("tool.pencilBrush", &json!({"preset": null})).unwrap();
    s.execute("edit.selectAll", &json!({})).ok();
    let before = s.doc().unwrap().nodes().count();
    s.pointer(&PointerEvent::at(PointerKind::Down, 0.0, 200.0, 1.0)).unwrap();
    s.pointer(&PointerEvent::at(PointerKind::Move, 50.0, 220.0, 1.0)).unwrap();
    s.pointer(&PointerEvent::at(PointerKind::Move, 100.0, 200.0, 1.0)).unwrap();
    s.pointer(&PointerEvent::at(PointerKind::Up, 100.0, 200.0, 1.0)).unwrap();
    let doc = s.doc().unwrap();
    assert_eq!(doc.nodes().count(), before + 1);
    let last = doc.nodes().filter(|n| matches!(n.kind, NodeKind::Vector { .. })).max_by_key(|n| n.id).unwrap();
    assert!(last.stroke.profile().is_none() && last.stroke.brush.is_none());
}

#[test]
fn drawing_tools_keep_independent_stroke_styles() {
    let mut s = Session::new();
    assert!(s.execute("tool.drawStyle", &json!({"tool": "brush", "color": "#FF0000", "preset": "missing"})).is_err());
    assert_eq!(s.tool.brush_style.color().to_hex(), "000000", "an invalid style does not change the tool");

    for (tool, color, weight, extra) in
        [("brush", "#FF0000", 19.0, json!({"preset": "ink"})), ("pencil", "#00CC66", 3.0, json!({"lineStyle": "dashed"}))]
    {
        s.execute("tool.set", &json!({"tool": tool})).unwrap();
        let mut style = json!({"tool": tool, "color": color, "weight": weight});
        style.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s.execute("tool.drawStyle", &style).unwrap();
        for (kind, x, y) in [(PointerKind::Down, 10.0, 30.0), (PointerKind::Move, 70.0, 60.0), (PointerKind::Up, 120.0, 30.0)] {
            s.pointer(&PointerEvent::at(kind, x, y, 1.0)).unwrap();
        }
    }

    let doc = s.doc().unwrap();
    let mut vectors = doc.nodes().filter(|n| matches!(n.kind, NodeKind::Vector { .. }));
    let brush = vectors.next().expect("brush stroke");
    let pencil = vectors.next().expect("pencil stroke");
    assert_eq!(brush.stroke.brush.as_ref().map(|b| b.preset.as_str()), Some("ink"));
    assert_eq!(brush.stroke.weight, 19.0);
    assert_eq!(brush.strokes.first().and_then(|p| p.solid_color()).map(|c| c.to_hex()).as_deref(), Some("FF0000"));
    assert!(pencil.stroke.brush.is_none());
    assert_eq!(pencil.stroke.weight, 3.0);
    assert!(!pencil.stroke.dashes.is_empty());
    assert_eq!(pencil.strokes.first().and_then(|p| p.solid_color()).map(|c| c.to_hex()).as_deref(), Some("00CC66"));
}

#[test]
fn eyedropper_updates_the_brush_color_and_returns_to_brush() {
    let mut s = Session::new();
    s.execute("node.create", &json!({"type": "rectangle", "x": 0, "y": 0, "width": 100, "height": 100, "props": {"fills": ["#3366CC"]}})).unwrap();
    s.execute("tool.set", &json!({"tool": "brush"})).unwrap();
    s.execute("edit.pickColor", &json!({"drawingTool": "brush"})).unwrap();
    s.pointer(&PointerEvent::at(PointerKind::Down, 50.0, 50.0, 1.0)).unwrap();
    assert_eq!(s.tool.tool.id(), "brush");
    assert_eq!(s.tool.brush_style.color().to_hex(), "3366CC");
}
