//! Vector edit mode through the session: tools, pointer gestures, keys and undo.

use serde_json::{Value, json};

use super::*;
use crate::EngineError;
use crate::tools::{PointerEvent, PointerKind};

pub(super) fn ex(s: &mut Session, c: &str, p: Value) -> Value {
    s.execute(c, &p).unwrap_or_else(|e| panic!("{c}: {e}"))
}

pub(super) fn net(s: &Session, id: NodeId) -> VectorNetwork {
    network(s.doc().unwrap(), id).unwrap().clone()
}

pub(super) fn ev(kind: PointerKind, x: f64, y: f64) -> PointerEvent {
    PointerEvent::at(kind, x, y, 1.0)
}

pub(super) fn click(s: &mut Session, x: f64, y: f64) {
    s.pointer(&ev(PointerKind::Down, x, y)).unwrap();
    s.pointer(&ev(PointerKind::Up, x, y)).unwrap();
}

pub(super) fn drag(s: &mut Session, from: (f64, f64), to: (f64, f64)) {
    s.pointer(&ev(PointerKind::Down, from.0, from.1)).unwrap();
    s.pointer(&ev(PointerKind::Move, (from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0)).unwrap();
    s.pointer(&ev(PointerKind::Move, to.0, to.1)).unwrap();
    s.pointer(&ev(PointerKind::Up, to.0, to.1)).unwrap();
}

/// A 100×100 rectangle at the origin, in edit mode.
pub(super) fn rect(s: &mut Session) -> NodeId {
    let r = ex(s, "shape.rectangle", json!({"x": 0, "y": 0, "width": 100, "height": 100}));
    let id = crate::params::id_of(&r["id"]).unwrap();
    ex(s, "vector.edit.begin", json!({"id": id}));
    id
}

pub(super) fn ve(s: &Session) -> VectorEdit {
    s.ds().unwrap().vector_edit.clone().unwrap()
}

pub(super) fn vertex_at(n: &VectorNetwork, x: f64, y: f64) -> usize {
    n.vertices.iter().position(|v| (v.x - x).abs() < 1e-6 && (v.y - y).abs() < 1e-6).unwrap()
}

pub(super) fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

#[test]
fn main_pen_button_keeps_the_active_vector_editable() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "tool.set", json!({"tool": "pen"}));
    assert_eq!(ve(&s).tool, VTool::Pen);
    assert_eq!(s.tool.tool, crate::tools::ToolId::Move, "the vector editor still owns pointer input");
    click(&mut s, 0.0, 0.0);
    click(&mut s, -80.0, -40.0);
    assert_eq!(net(&s, id).vertices.len(), 5);
    assert_eq!(s.doc().unwrap().node(s.page().unwrap()).unwrap().children.len(), 1);
    ex(&mut s, "tool.set", json!({"tool": "move"}));
    assert_eq!(ve(&s).tool, VTool::Move);
    ex(&mut s, "tool.set", json!({"tool": "pen"}));
    assert_eq!(ve(&s).tool, VTool::Pen);
}

#[test]
fn pen_snaps_to_a_crossing_and_creates_one_shared_vertex() {
    let mut s = Session::new();
    let made = ex(&mut s, "vector.create", json!({"points": [[0, 0], [100, 100], [0, 100], [100, 0]], "closed": false}));
    let id = crate::params::id_of(&made["id"]).unwrap();
    ex(&mut s, "vector.edit.begin", json!({"id": id}));
    ex(&mut s, "tool.set", json!({"tool": "pen"}));
    let crossing = super::point_snap::intersection_near(s.doc().unwrap(), id, pt(52.0, 49.0), 8.0).unwrap();
    assert!((crossing.at - pt(50.0, 50.0)).hypot() < 1e-6);
    assert!(super::point_snap::intersection_near(s.doc().unwrap(), id, pt(65.0, 50.0), 8.0).is_none());
    s.pointer(&ev(PointerKind::Hover, 52.0, 49.0)).unwrap();
    assert!(s.tool.overlay.guides.iter().any(|guide| (guide.x0 - 50.0).abs() < 1e-6 && (guide.y0 - 50.0).abs() < 1e-6));
    click(&mut s, 52.0, 49.0);
    let n = net(&s, id);
    let world = s.doc().unwrap().world_transform(id);
    let junction = n.vertices.iter().position(|v| (world * v.point() - pt(50.0, 50.0)).hypot() < 1e-6).unwrap();
    assert_eq!(n.vertices.len(), 5);
    assert_eq!(n.incident(junction).len(), 4, "both crossing lines share the new point");
    click(&mut s, 50.0, 50.0);
    click(&mut s, 50.0, 160.0);
    assert_eq!(net(&s, id).vertices.len(), 6, "the pen extends the same vector from the junction");
    assert_eq!(s.doc().unwrap().node(s.page().unwrap()).unwrap().children.len(), 1);
}

#[test]
fn shift_pen_preview_and_click_share_the_same_45_degree_point() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.tool", json!({"tool": "pen"}));
    click(&mut s, 100.0, 0.0);
    let mut event = ev(PointerKind::Hover, 143.0, 37.0);
    event.shift = true;
    s.pointer(&event).unwrap();
    assert_eq!(s.tool.overlay.rubber, Some([100.0, 0.0, 140.0, 40.0]));
    assert_eq!(s.tool.overlay.angle_guide, s.tool.overlay.rubber);
    event.kind = PointerKind::Down;
    s.pointer(&event).unwrap();
    event.kind = PointerKind::Up;
    s.pointer(&event).unwrap();
    let n = net(&s, id);
    let world = s.doc().unwrap().world_transform(id);
    assert!(n.vertices.iter().any(|v| (world * v.point() - pt(140.0, 40.0)).hypot() < 1e-6));
    assert!(s.tool.overlay.angle_guide.is_none());
}

#[test]
fn shift_dragging_a_selected_point_snaps_diagonal_and_can_unlock_mid_drag() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [0]}));
    let mut event = ev(PointerKind::Down, 0.0, 0.0);
    event.shift = true;
    s.pointer(&event).unwrap();
    event.kind = PointerKind::Move;
    event.x = 37.0;
    event.y = 29.0;
    s.pointer(&event).unwrap();
    assert_eq!(s.tool.overlay.angle_guide, Some([0.0, 0.0, 33.0, 33.0]));
    let world = s.doc().unwrap().world_transform(id);
    assert!((world * net(&s, id).vertices[0].point() - pt(33.0, 33.0)).hypot() < 1e-6);
    event.shift = false;
    event.x = 35.0;
    event.y = 20.0;
    s.pointer(&event).unwrap();
    assert!(s.tool.overlay.angle_guide.is_none());
    event.shift = true;
    event.x = 3.0;
    event.y = 35.0;
    s.pointer(&event).unwrap();
    assert_eq!(s.tool.overlay.angle_guide, Some([0.0, 0.0, 0.0, 35.0]));
    event.kind = PointerKind::Up;
    s.pointer(&event).unwrap();
    let world = s.doc().unwrap().world_transform(id);
    assert!((world * net(&s, id).vertices[0].point() - pt(0.0, 35.0)).hypot() < 1e-6);
    assert!(s.tool.overlay.angle_guide.is_none());
    assert!(s.undo().unwrap());
    let world = s.doc().unwrap().world_transform(id);
    assert!((world * net(&s, id).vertices[0].point() - pt(0.0, 0.0)).hypot() < 1e-6);
}

#[test]
fn shift_click_still_removes_a_selected_point_without_moving_it() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [0]}));
    let mut event = ev(PointerKind::Down, 0.0, 0.0);
    event.shift = true;
    s.pointer(&event).unwrap();
    event.kind = PointerKind::Up;
    s.pointer(&event).unwrap();
    assert!(ve(&s).vertices.is_empty());
    assert_eq!(net(&s, id).vertices.len(), 4);
}

#[test]
fn shift_also_constrains_the_initial_pen_tool() {
    let mut s = Session::new();
    s.set_tool(crate::tools::ToolId::Pen);
    click(&mut s, 100.0, 100.0);
    let mut event = ev(PointerKind::Hover, 134.0, 127.0);
    event.shift = true;
    s.pointer(&event).unwrap();
    assert_eq!(s.tool.overlay.rubber, Some([100.0, 100.0, 130.5, 130.5]));
    assert_eq!(s.tool.overlay.angle_guide, s.tool.overlay.rubber);
    event.kind = PointerKind::Down;
    s.pointer(&event).unwrap();
    event.kind = PointerKind::Up;
    s.pointer(&event).unwrap();
    assert_eq!(s.tool.pen.as_ref().and_then(|pen| pen.points.last()).map(|(p, _, _)| *p), Some(pt(130.5, 130.5)));
    assert!(s.tool.overlay.angle_guide.is_none());
}

#[test]
fn corner_radius_updates_only_selected_points_and_undoes_together() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [0, 2]}));
    ex(&mut s, "vector.edit.cornerRadius", json!({"radius": 8}));
    let radii = |s: &Session| net(s, id).vertices.iter().map(|v| v.radius).collect::<Vec<_>>();
    assert_eq!(radii(&s), vec![8.0, 0.0, 8.0, 0.0]);
    ex(&mut s, "vector.edit.select", json!({"vertices": [0, 1]}));
    ex(&mut s, "vector.edit.cornerRadius", json!({"add": 2}));
    assert_eq!(radii(&s), vec![10.0, 2.0, 8.0, 0.0]);
    assert_eq!(s.doc().unwrap().node(id).unwrap().corners.radii, [0.0; 4], "the layer radius stays separate");
    assert!(s.undo().unwrap());
    assert_eq!(radii(&s), vec![8.0, 0.0, 8.0, 0.0]);
    ex(&mut s, "vector.edit.cornerRadius", json!({"multiply": 2}));
    assert_eq!(radii(&s), vec![16.0, 0.0, 8.0, 0.0]);
    assert!(s.execute("vector.edit.cornerRadius", &json!({"divide": 0})).is_err());
}

#[test]
fn pen_extends_branches_and_closes_paths() {
    let mut s = Session::new();
    let r = ex(&mut s, "vector.create", json!({"points": [[0, 0], [100, 0]], "closed": false}));
    let id = crate::params::id_of(&r["id"]).unwrap();
    ex(&mut s, "vector.edit.begin", json!({"id": id}));
    ex(&mut s, "vector.edit.tool", json!({"tool": "pen"}));
    // Click the end point to continue from it, then add a point below and close onto the start.
    click(&mut s, 100.0, 0.0);
    assert!(ve(&s).anchor.is_some());
    click(&mut s, 100.0, 100.0);
    let n = net(&s, id);
    assert_eq!((n.vertices.len(), n.segments.len()), (3, 2));
    click(&mut s, 0.0, 0.0);
    let n = net(&s, id);
    assert_eq!(n.segments.len(), 3);
    assert_eq!(n.regions.len(), 1, "closing the path makes a fill region");
    assert!(ve(&s).anchor.is_none(), "closing ends the path");
    // Branch from an existing corner.
    click(&mut s, 100.0, 100.0);
    click(&mut s, 200.0, 150.0);
    let n = net(&s, id);
    let corner = vertex_at(&n, 100.0, 100.0);
    assert_eq!(n.incident(corner).len(), 3, "a vector network branch");
    // Escape ends the path but stays in edit mode; the next Escape leaves.
    s.escape().unwrap();
    assert!(ve(&s).anchor.is_none());
    s.escape().unwrap();
    assert!(s.ds().unwrap().vector_edit.is_none());
}

#[test]
fn pen_drag_makes_a_smooth_point() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.tool", json!({"tool": "pen"}));
    click(&mut s, 100.0, 0.0);
    drag(&mut s, (150.0, 50.0), (150.0, 80.0));
    let n = net(&s, id);
    let v = n.vertices.len() - 1;
    assert_eq!(n.mirroring(v), HandleMirroring::AngleAndLength);
    let (seg, start) = n.incident(v)[0];
    let h = n.handle(seg, start).unwrap();
    assert!(h.0.abs() < 1e-6 && (h.1 + 30.0).abs() < 1e-6, "incoming handle mirrors the drag: {h:?}");
    assert_eq!(ve(&s).pen_out, Some((0.0, 30.0)));
    // The next segment starts with the dragged-out handle.
    click(&mut s, 250.0, 50.0);
    let n = net(&s, id);
    let last = *n.segments.last().unwrap();
    assert_eq!(last.ta, (0.0, 30.0));
    // One undo step per click.
    assert!(s.undo().unwrap());
    assert_eq!(net(&s, id).vertices.len(), n.vertices.len() - 1);
}

#[test]
fn bend_tool_smooths_and_sharpens_points() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.tool", json!({"tool": "bend"}));
    drag(&mut s, (100.0, 0.0), (130.0, 0.0));
    let n = net(&s, id);
    let v = vertex_at(&n, 100.0, 0.0);
    assert_eq!(n.mirroring(v), HandleMirroring::Angle);
    assert!(n.incident(v).iter().all(|(s, st)| n.handle(*s, *st) != Some((0.0, 0.0))));
    click(&mut s, 100.0, 0.0);
    let n = net(&s, id);
    let v = vertex_at(&n, 100.0, 0.0);
    assert!(n.incident(v).iter().all(|(s, st)| n.handle(*s, *st) == Some((0.0, 0.0))), "a click sharpens it");
    // ⌘-drag on a segment with the Move tool bends it.
    ex(&mut s, "vector.edit.tool", json!({"tool": "move"}));
    let mut down = ev(PointerKind::Down, 0.0, 50.0);
    down.cmd = true;
    s.pointer(&down).unwrap();
    s.pointer(&ev(PointerKind::Move, -40.0, 50.0)).unwrap();
    s.pointer(&ev(PointerKind::Up, -40.0, 50.0)).unwrap();
    let b = s.doc().unwrap().world_bounds(id).unwrap();
    assert!(b.x0 < -25.0, "bent outwards: {b:?}");
}

#[test]
fn paint_bucket_fills_and_unfills_areas() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.connect", json!({"a": 0, "b": 2}));
    ex(&mut s, "vector.edit.tool", json!({"tool": "paint"}));
    assert!(net(&s, id).fill_contains(pt(75.0, 25.0)));
    // Hovering highlights the area; clicking unfills just that triangle.
    s.pointer(&ev(PointerKind::Hover, 75.0, 25.0)).unwrap();
    assert!(!s.tool.overlay.face.is_empty());
    click(&mut s, 75.0, 25.0);
    let n = net(&s, id);
    assert!(!n.fill_contains(pt(75.0, 25.0)));
    assert!(n.fill_contains(pt(25.0, 75.0)));
    click(&mut s, 75.0, 25.0);
    assert!(net(&s, id).fill_contains(pt(75.0, 25.0)));
    s.undo().unwrap();
    assert!(!net(&s, id).fill_contains(pt(75.0, 25.0)));
    // A vector without fills gets one when painted.
    ex(&mut s, "node.set", json!({"id": id, "props": {"fills": []}}));
    let r = ex(&mut s, "vector.edit.paint", json!({"x": 75, "y": 25}));
    assert_eq!(r["filled"], json!(true));
    assert!(!s.doc().unwrap().node(id).unwrap().fills.is_empty());
    assert!(s.execute("vector.edit.paint", &json!({"x": 500, "y": 500})).is_err(), "outside any area");
}

#[test]
fn paint_bucket_can_fill_each_face_of_a_connected_pen_network() {
    let mut s = Session::new();
    let id = rect(&mut s);
    let points = [
        (264.43535702309975, 102.54619315734607),
        (264.43535702309975, 183.9741802735134),
        (0.0, 183.9741802735134),
        (0.0, 0.0),
        (102.54619315734612, 102.54619315734605),
        (183.97418027351347, 183.9741802735134),
        (0.0, 102.54619315734608),
    ];
    let edges = [(0, 1), (1, 5), (2, 6), (3, 4), (4, 0), (4, 5), (5, 2), (6, 4), (6, 3)];
    super::edit(s.doc_mut().unwrap(), id, |network| {
        network.vertices = points.into_iter().map(|(x, y)| Vertex::new(x, y)).collect();
        network.segments = edges.into_iter().map(|(a, b)| Segment::line(a, b)).collect();
        network.regions.clear();
    })
    .unwrap();
    // This is an unpainted pen network; each bucket click should fill only its chosen face.
    s.doc_mut().unwrap().get_mut(id).unwrap().fills.clear();
    ex(&mut s, "vector.edit.tool", json!({"tool": "paint"}));
    for sample in [pt(20.0, 60.0), pt(40.0, 150.0), pt(220.0, 150.0)] {
        s.pointer(&ev(PointerKind::Hover, sample.x, sample.y)).unwrap();
        assert!(!s.tool.overlay.face.is_empty(), "no hover face at {sample:?}");
        click(&mut s, sample.x, sample.y);
        assert!(net(&s, id).fill_contains(sample), "paint did not fill {sample:?}");
    }
}

#[test]
fn lasso_marquee_and_segment_selection() {
    let mut s = Session::new();
    let _id = rect(&mut s);
    ex(&mut s, "vector.edit.tool", json!({"tool": "lasso"}));
    s.pointer(&ev(PointerKind::Down, -10.0, -10.0)).unwrap();
    for (x, y) in [(110.0, -10.0), (110.0, 10.0), (-10.0, 10.0)] {
        s.pointer(&ev(PointerKind::Move, x, y)).unwrap();
    }
    assert!(!s.tool.overlay.lasso.is_empty());
    s.pointer(&ev(PointerKind::Up, -10.0, 10.0)).unwrap();
    assert_eq!(ve(&s).vertices.len(), 2, "the top two points");
    assert!(s.tool.overlay.lasso.is_empty());
    // Move tool: clicking a segment selects it; shift adds another.
    ex(&mut s, "vector.edit.tool", json!({"tool": "move"}));
    click(&mut s, 50.0, 100.0);
    assert_eq!(ve(&s).segments.len(), 1);
    assert!(ve(&s).vertices.is_empty());
    let mut sh = ev(PointerKind::Down, 0.0, 50.0);
    sh.shift = true;
    s.pointer(&sh).unwrap();
    s.pointer(&ev(PointerKind::Up, 0.0, 50.0)).unwrap();
    assert_eq!(ve(&s).segments.len(), 2);
    ex(&mut s, "vector.edit.selectAll", json!({}));
    assert_eq!(ve(&s).vertices.len(), 4);
}

#[test]
fn marquee_can_start_outside_the_vector_and_only_double_click_exits() {
    let mut s = Session::new();
    let _id = rect(&mut s);

    click(&mut s, -100.0, -100.0);
    assert!(s.ds().unwrap().vector_edit.is_some(), "a single click outside keeps edit mode");

    s.pointer(&ev(PointerKind::Down, -30.0, -30.0)).unwrap();
    s.pointer(&ev(PointerKind::Move, 20.0, 20.0)).unwrap();
    assert!(s.tool.overlay.marquee.is_some(), "a drag from outside starts a marquee");
    s.pointer(&ev(PointerKind::Up, 20.0, 20.0)).unwrap();
    assert_eq!(ve(&s).vertices.len(), 1, "the marquee selects the corner point");

    s.pointer(&ev(PointerKind::DoubleClick, 50.0, 50.0)).unwrap();
    assert!(s.ds().unwrap().vector_edit.is_some(), "a double-click inside the vector keeps edit mode");

    s.pointer(&ev(PointerKind::Down, -100.0, -100.0)).unwrap();
    s.pointer(&ev(PointerKind::DoubleClick, -100.0, -100.0)).unwrap();
    assert!(s.ds().unwrap().vector_edit.is_none(), "a double-click outside ends edit mode");
    assert!(!s.tool.busy(), "leaving edit mode clears the pending marquee gesture");
}

#[test]
fn double_click_selects_connected_subpath_and_alt_selects_only_the_closed_loop() {
    let mut s = Session::new();
    let id = rect(&mut s);
    let mut n = VectorNetwork::from_points(&[pt(0.0, 0.0), pt(200.0, 0.0), pt(200.0, 200.0), pt(0.0, 200.0)], true);
    let inner = [pt(60.0, 60.0), pt(140.0, 60.0), pt(140.0, 140.0), pt(60.0, 140.0)];
    for point in inner {
        n.vertices.push(Vertex::new(point.x, point.y));
    }
    for i in 0..4 {
        n.segments.push(Segment::line(4 + i, 4 + (i + 1) % 4));
    }
    n.regions[0].loops.push(vec![4, 5, 6, 7]);
    n.regions[0].winding = WindingRule::EvenOdd;
    n.vertices.push(Vertex::new(100.0, 100.0));
    n.segments.push(Segment::line(4, 8));
    let doc = s.doc_mut().unwrap();
    let node = doc.get_mut(id).unwrap();
    node.kind = NodeKind::Vector { network: n };
    node.transform = Affine::IDENTITY;
    super::refit(doc, id).unwrap();
    ex(&mut s, "vector.edit.begin", json!({"id": id}));
    let before = net(&s, id);
    let undo_steps = s.ds().unwrap().history.undo.len();

    s.pointer(&ev(PointerKind::DoubleClick, 100.0, 60.0)).unwrap();
    assert_eq!(ve(&s).segments, vec![4, 5, 6, 7, 8], "plain double-click follows the connected branch");
    assert!(ve(&s).vertices.is_empty());

    let mut alt = ev(PointerKind::DoubleClick, 100.0, 60.0);
    alt.alt = true;
    s.pointer(&alt).unwrap();
    assert_eq!(ve(&s).segments, vec![4, 5, 6, 7], "Alt selects just the inner closed contour");

    alt.x = 100.0;
    alt.y = 0.0;
    alt.shift = true;
    s.pointer(&alt).unwrap();
    assert_eq!(ve(&s).segments, (0..8).collect::<Vec<_>>(), "Shift+Alt adds the outer contour");

    ex(&mut s, "vector.edit.selectOuterContour", json!({}));
    assert_eq!(ve(&s).segments, vec![0, 1, 2, 3]);
    ex(&mut s, "vector.edit.selectInnerContours", json!({}));
    assert_eq!(ve(&s).segments, vec![4, 5, 6, 7]);
    assert_eq!(net(&s, id), before, "selection never edits the network");
    assert_eq!(s.ds().unwrap().history.undo.len(), undo_steps, "selection creates no undo step");
}

#[test]
fn alt_click_adds_a_point_that_drags_in_the_same_gesture() {
    let mut s = Session::new();
    let id = rect(&mut s);
    let mut down = ev(PointerKind::Down, 50.0, 0.0);
    down.alt = true;
    s.pointer(&down).unwrap();
    s.pointer(&ev(PointerKind::Move, 50.0, -20.0)).unwrap();
    s.pointer(&ev(PointerKind::Up, 50.0, -20.0)).unwrap();
    assert_eq!(net(&s, id).vertices.len(), 5);
    let b = s.doc().unwrap().world_bounds(id).unwrap();
    assert_eq!(b.y0, -20.0, "the new point was dragged up");
    s.undo().unwrap();
    assert_eq!(net(&s, id).vertices.len(), 4, "add + drag is one step");
}

#[test]
fn cut_tool_and_keys() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.tool", json!({"tool": "cut"}));
    click(&mut s, 50.0, 0.0);
    assert_eq!(net(&s, id).vertices.len(), 6);
    assert_eq!(ve(&s).vertices.len(), 2);
    // Keys: tool letters, arrows nudge, ⌘A selects all, Delete removes, Enter leaves.
    assert!(key(&mut s, "V", false, false, false).unwrap());
    assert_eq!(ve(&s).tool, VTool::Move);
    assert!(key(&mut s, "Q", false, false, false).unwrap());
    assert_eq!(ve(&s).tool, VTool::Lasso);
    assert!(key(&mut s, "B", true, false, false).unwrap());
    assert_eq!(ve(&s).tool, VTool::Paint);
    let v = vertex_at(&net(&s, id), 100.0, 100.0);
    ex(&mut s, "vector.edit.select", json!({"vertices": [v]}));
    assert!(key(&mut s, "Right", false, false, false).unwrap());
    let w = s.doc().unwrap().world_transform(id);
    assert_eq!((w * net(&s, id).vertices[v].point()).x, 101.0);
    assert!(key(&mut s, "Down", true, false, false).unwrap());
    let w = s.doc().unwrap().world_transform(id);
    assert_eq!((w * net(&s, id).vertices[v].point()).y, 110.0);
    assert!(key(&mut s, "A", false, false, true).unwrap());
    assert_eq!(ve(&s).vertices.len(), 6);
    assert!(!key(&mut s, "Z", false, false, true).unwrap(), "undo falls through to shortcuts");
    ex(&mut s, "vector.edit.select", json!({"vertices": [v]}));
    assert!(key(&mut s, "Backspace", false, false, false).unwrap());
    assert_eq!(net(&s, id).vertices.len(), 5);
    assert!(key(&mut s, "Enter", false, false, false).unwrap());
    assert!(s.ds().unwrap().vector_edit.is_none());
    assert!(!key(&mut s, "V", false, false, false).unwrap(), "keys pass through outside edit mode");
}

#[test]
fn delete_vs_delete_and_heal() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [1]}));
    ex(&mut s, "vector.edit.delete", json!({}));
    let n = net(&s, id);
    assert_eq!((n.vertices.len(), n.segments.len()), (3, 2), "the path opens");
    s.undo().unwrap();
    ex(&mut s, "vector.edit.select", json!({"vertices": [1]}));
    ex(&mut s, "vector.deleteHeal", json!({}));
    let n = net(&s, id);
    assert_eq!((n.vertices.len(), n.segments.len(), n.regions.len()), (3, 3, 1), "healed and closed");
    // Deleting a selected segment keeps its points.
    s.undo().unwrap();
    ex(&mut s, "vector.edit.select", json!({"segments": [0]}));
    ex(&mut s, "vector.edit.delete", json!({}));
    let n = net(&s, id);
    assert_eq!((n.vertices.len(), n.segments.len()), (4, 3));
}

#[test]
fn mirroring_position_and_join_commands() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [1]}));
    ex(&mut s, "vector.edit.smooth", json!({}));
    ex(&mut s, "vector.edit.mirroring", json!({"mode": "angleAndLength"}));
    let st = ex(&mut s, "vector.edit.state", json!({}));
    assert_eq!(st["mirroring"][1], json!("angleAndLength"));
    assert_eq!(st["tool"], json!("move"));
    assert!(s.execute("vector.edit.mirroring", &json!({"mode": "sideways"})).is_err());
    // Position: move the selected point to x = -50 in parent coordinates.
    ex(&mut s, "vector.edit.select", json!({"vertices": [0]}));
    ex(&mut s, "vector.edit.position", json!({"x": -50}));
    let p = selection_position(s.doc().unwrap(), &ve(&s)).unwrap();
    assert!((p.x + 50.0).abs() < 1e-6, "{p:?}");
    // Join: cut a corner open, then join its two ends back (they overlap: merged).
    ex(&mut s, "vector.edit.cut", json!({"vertex": 2}));
    let before = net(&s, id).vertices.len();
    ex(&mut s, "vector.join", json!({}));
    assert_eq!(net(&s, id).vertices.len(), before - 1);
    // Outside edit mode, vector.join keeps its layer behaviour.
    ex(&mut s, "vector.edit.end", json!({}));
    assert!(s.execute("vector.join", &json!({"ids": [id]})).is_ok());
}

#[test]
fn aligning_points_uses_selection_bounds_and_preserves_undo() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [0, 2]}));
    ex(&mut s, "vector.edit.align", json!({"mode": "hcenter"}));
    let aligned = net(&s, id);
    assert!((aligned.vertices[0].x - aligned.vertices[2].x).abs() < 1e-6);
    assert!(s.undo().unwrap());
    let restored = net(&s, id);
    assert!((restored.vertices[0].x - restored.vertices[2].x).abs() > 1.0);

    ex(&mut s, "vector.edit.select", json!({"vertices": [0]}));
    ex(&mut s, "vector.edit.align", json!({"mode": "right"}));
    let single = net(&s, id);
    let right = single.vertices.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
    assert!((single.vertices[0].x - right).abs() < 1e-6);
    assert!(s.execute("vector.edit.align", &json!({"mode": "diagonal"})).is_err());
}

#[test]
fn distribute_shortcuts_space_selected_points_without_moving_the_layer() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.select", json!({"vertices": [0, 1, 2]}));
    let before_tidy = net(&s, id);
    ex(&mut s, "arrange.tidy", json!({}));
    assert_eq!(net(&s, id), before_tidy, "Tidy is disabled for vector points");
    ex(&mut s, "distribute.horizontal", json!({}));
    let points = net(&s, id).vertices;
    let mut selected = [points[0].x, points[1].x, points[2].x];
    selected.sort_by(f64::total_cmp);
    assert_eq!(selected, [0.0, 50.0, 100.0]);
    assert_eq!(points[3].x, 0.0);
    assert!(s.undo().unwrap());
    assert_eq!(net(&s, id).vertices[1].x, 100.0);

    ex(&mut s, "vector.edit.select", json!({"vertices": [0, 2, 3]}));
    ex(&mut s, "distribute.vertical", json!({}));
    let points = net(&s, id).vertices;
    let mut selected = [points[0].y, points[2].y, points[3].y];
    selected.sort_by(f64::total_cmp);
    assert_eq!(selected, [0.0, 50.0, 100.0]);
    assert_eq!(points[1].y, 0.0);
}

#[test]
fn a_point_dropped_on_another_merges_into_it() {
    // #401: an open path's end dragged onto its start closes the path at that point.
    let mut s = Session::new();
    let r = ex(&mut s, "vector.create", json!({"points": [[0, 0], [100, 0], [100, 100], [0, 100]]}));
    let id = crate::params::id_of(&r["id"]).unwrap();
    ex(&mut s, "vector.edit.begin", json!({"id": id}));
    drag(&mut s, (0.0, 100.0), (2.0, 3.0));
    let n = net(&s, id);
    assert_eq!(n.vertices.len(), 3, "the dropped point merged");
    assert!((0..3).all(|v| n.incident(v).len() == 2), "a closed path: {n:?}");
    assert_eq!(ve(&s).vertices, vec![vertex_at(&n, 0.0, 0.0)], "the point it merged into stays selected");
    s.undo().unwrap();
    let n = net(&s, id);
    assert_eq!(n.vertices.len(), 4, "one undo step takes back the move and the merge");
    vertex_at(&n, 0.0, 100.0);
    // A drop away from every point only moves it.
    ex(&mut s, "vector.edit.select", json!({"vertices": []}));
    drag(&mut s, (0.0, 100.0), (0.0, 50.0));
    assert_eq!(net(&s, id).vertices.len(), 4);
    // A 2-point line's end dropped on its other end only moves: no segment would be left.
    let mut s = Session::new();
    let r = ex(&mut s, "vector.create", json!({"points": [[0, 0], [100, 0]]}));
    let id = crate::params::id_of(&r["id"]).unwrap();
    ex(&mut s, "vector.edit.begin", json!({"id": id}));
    drag(&mut s, (100.0, 0.0), (2.0, 0.0));
    let n = net(&s, id);
    assert_eq!((n.vertices.len(), n.segments.len()), (2, 1), "still a line: {n:?}");
}

#[test]
fn undo_back_past_the_conversion_leaves_edit_mode() {
    let mut s = Session::new();
    let id = rect(&mut s);
    assert!(s.ds().unwrap().vector_edit.is_some());
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().node(id).unwrap().kind.type_name(), "RECTANGLE");
    assert!(s.ds().unwrap().vector_edit.is_none());
    // Choosing a drawing tool leaves edit mode too.
    let _ = rect(&mut s);
    s.set_tool(crate::tools::ToolId::Rectangle);
    assert!(s.ds().unwrap().vector_edit.is_none());
}

#[test]
fn every_command_survives_junk_params_while_editing() {
    let junk = [
        json!({}),
        json!(null),
        json!({"vertices": [-1, 1e12, "x"], "segments": [99999], "segment": -5, "t": "NaN", "x": 1e308, "y": -1e308}),
        json!({"segment": 0, "t": 2, "x": 5, "y": 5, "dx": 1e300, "dy": 1, "a": 0, "b": 0, "vertex": 99}),
        json!({"points": [[0, 0], [1], "x", [5, 5], [0, 9]], "mode": 7, "tool": "nope", "radius": -3}),
    ];
    for spec in crate::all_commands().iter().filter(|c| c.id.starts_with("vector.")) {
        for j in &junk {
            let mut s = Session::new();
            let _ = rect(&mut s);
            if let Err(EngineError::Internal { cmd, msg }) = s.execute(spec.id, j) {
                panic!("{cmd} panicked on {j}: {msg}");
            }
            // The network stays sane.
            if let Some(ve) = s.ds().unwrap().vector_edit.clone()
                && let Some(n) = network(s.doc().unwrap(), ve.id)
            {
                assert!(n.segments.iter().all(|x| x.a < n.vertices.len() && x.b < n.vertices.len()), "{} broke the network", spec.id);
            }
        }
    }
}

#[test]
fn eraser_strokes_cut_paths_in_one_step() {
    let mut s = Session::new();
    let id = rect(&mut s);
    ex(&mut s, "vector.edit.tool", json!({"tool": "eraser"}));
    s.pointer(&ev(PointerKind::Hover, 50.0, 0.0)).unwrap();
    assert!(s.tool.overlay.brush.is_some(), "the brush shows");
    // A stroke across the top edge.
    drag(&mut s, (50.0, -5.0), (50.0, 5.0));
    let n = net(&s, id);
    assert_eq!(n.segments.len(), 5, "the top edge is split into two with a gap");
    assert!(n.vertices.iter().all(|v| (v.point() - pt(50.0, 0.0)).hypot() >= 7.9), "nothing left under the brush");
    s.undo().unwrap();
    assert_eq!(net(&s, id).segments.len(), 4);
    assert_eq!(net(&s, id).vertices.len(), 4, "one undo step restores the square");
    // The command does the same with an explicit radius.
    let r = ex(&mut s, "vector.edit.erase", json!({"points": [[100, 50]], "radius": 5}));
    assert_eq!(r["changed"], json!(true));
    assert!(key(&mut s, "E", true, false, false).unwrap());
    assert_eq!(ve(&s).tool, VTool::Eraser);
}
