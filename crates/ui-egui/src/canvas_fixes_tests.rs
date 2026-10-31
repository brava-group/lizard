//! Canvas fixes, driven through egui headless: View ▸ Frame outlines and View ▸ Slices draw (off
//! by default), the render worker counts a job as in flight until it is done, and the Pen shows
//! the handles of the path it draws.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Shape};
use figcraft_color::Color;
use figcraft_doc::{GridAlign, GridKind, LayoutGrid};
use figcraft_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;
use crate::live_tests::flat;
use crate::present_tests::id;

/// Whether `shapes` hold a closed outline through the corners `q`.
fn outlined(shapes: &[Shape], q: [Pos2; 4]) -> bool {
    shapes.iter().any(|s| matches!(s, Shape::Path(p) if p.closed && p.points.len() == 4 && p.points.iter().zip(q).all(|(a, b)| a.distance(b) < 0.5)))
}

/// How many line segments lie along the top edge from `a` to `b` (a dashed outline's dashes).
fn dashes_along(shapes: &[Shape], a: Pos2, b: Pos2) -> usize {
    let on = |p: Pos2| (p.y - a.y).abs() < 0.5 && p.x >= a.x - 0.5 && p.x <= b.x + 0.5;
    shapes.iter().filter(|s| matches!(s, Shape::LineSegment { points: [p, q], .. } if on(*p) && on(*q))).count()
}

#[test]
fn layout_grid_tracks_are_clipped_to_their_frame() {
    let mut s = Session::new();
    let frame = id(&s.execute("frame.create", &json!({"x": 100, "y": 100, "width": 32, "height": 32})).unwrap());
    let grid = LayoutGrid {
        kind: GridKind::Rows,
        align: GridAlign::Min,
        count: 5,
        size: 4.0,
        gutter: 24.0,
        color: Color::new(1.0, 0.0, 0.0, 0.1),
        ..LayoutGrid::default()
    };
    s.doc_mut().unwrap().get_mut(frame).unwrap().grids = vec![grid.clone(), LayoutGrid { kind: GridKind::Columns, ..grid.clone() }];
    s.execute("view.layoutGrids", &json!({"on": true})).unwrap();
    let mut ui = Ui::new(s);
    let bounds = Rect::from_two_pos(ui.app.to_screen(100.0, 100.0), ui.app.to_screen(132.0, 132.0));
    assert!(ui.app.canvas.rect.contains_rect(bounds), "the frame is visible on the test canvas");
    let color = crate::theme::doc_c32(&ui.ctx, grid.color);
    let output = ui.frame_output(vec![]);
    let grid_clips: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == color).then_some(clipped.clip_rect))
        .collect();
    assert_eq!(grid_clips.len(), 10, "all configured rows and columns still exist");
    for clip in grid_clips {
        assert!((clip.min - bounds.min).length() < 0.5 && (clip.max - bounds.max).length() < 0.5, "grid clip {clip:?} exceeds frame {bounds:?}");
    }
}

#[test]
fn frame_outlines_and_slices_draw_when_shown() {
    let mut s = Session::new();
    let f = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 200, "height": 120})).unwrap());
    let inner = id(&s.execute("frame.create", &json!({"x": 20, "y": 20, "width": 80, "height": 60, "parent": f})).unwrap());
    let slice = id(&s.execute("node.create", &json!({"type": "slice", "x": 300, "y": 0, "width": 100, "height": 50})).unwrap());
    s.execute("select.set", &json!({"ids": []})).unwrap();
    let mut u = Ui::new(s);
    let quad = |u: &Ui, i| crate::canvas::node_quad(&u.app, u.app.session.doc().unwrap(), i).unwrap();
    let shapes = flat(&u.frame_output(vec![]).shapes);
    let sq = quad(&u, slice);
    assert!(!outlined(&shapes, quad(&u, inner)), "off by default");
    assert_eq!(dashes_along(&shapes, sq[0], sq[1]), 0, "slices hidden by default");
    u.app.run("view.frameOutlines", json!({}));
    u.app.run("view.slices", json!({}));
    let shapes = flat(&u.frame_output(vec![]).shapes);
    for frame in [f, inner] {
        assert!(outlined(&shapes, quad(&u, frame)), "every frame is outlined");
    }
    assert!(dashes_along(&shapes, sq[0], sq[1]) > 3, "the slice shows as a dashed rectangle");
    u.app.run("view.frameOutlines", json!({"on": false}));
    u.app.run("view.slices", json!({"on": false}));
    let shapes = flat(&u.frame_output(vec![]).shapes);
    assert!(!outlined(&shapes, quad(&u, inner)) && dashes_along(&shapes, sq[0], sq[1]) == 0, "both turn off again");
}

#[test]
fn open_vector_hover_and_selection_show_the_path_and_selection_box() {
    let mut s = Session::new();
    let vector = id(&s
        .execute(
            "vector.create",
            &json!({
                "path": "M0 120 L100 0 L200 120", "x": 80, "y": 80,
                "props": {"fills": [], "strokes": ["#000000"], "strokeWeight": 1},
            }),
        )
        .unwrap());
    s.execute("select.none", &json!({})).unwrap();
    let mut u = Ui::new(s);
    let blue = egui::epaint::ColorMode::Solid(crate::theme::ACCENT);
    let blue_path = |shapes: &[Shape]| {
        shapes.iter().any(|shape| {
            matches!(shape, Shape::Path(path)
            if !path.closed && path.points.len() >= 3 && path.stroke.color == blue)
        })
    };
    let hover = u.app.to_screen(130.0, 140.0);
    let shapes = flat(&u.frame_output(vec![egui::Event::PointerMoved(hover)]).shapes);
    assert_eq!(u.app.session.ds().unwrap().hover, Some(vector));
    assert!(blue_path(&shapes), "hover outlines the open vector, not its empty fill path");

    u.app.run("select.set", json!({"ids": [vector]}));
    let shapes = flat(&u.frame_output(vec![]).shapes);
    let doc = u.app.session.doc().unwrap();
    let box_corners = crate::canvas::node_quad(&u.app, doc, vector).unwrap();
    assert!(blue_path(&shapes), "selection outlines the vector's full stroke path");
    assert!(outlined(&shapes, box_corners), "selection draws all four sides of the vector's box");
}

#[test]
fn selected_curved_and_polygonal_shapes_show_their_transform_box() {
    for kind in ["shape.ellipse", "shape.polygon", "shape.star"] {
        let mut session = Session::new();
        let shape = id(&session.execute(kind, &json!({"x": 80, "y": 80, "width": 120, "height": 100})).unwrap());
        session.execute("select.set", &json!({"ids": [shape]})).unwrap();
        let mut ui = Ui::new(session);
        let shapes = flat(&ui.frame_output(vec![]).shapes);
        let corners = crate::canvas::node_quad(&ui.app, ui.app.session.doc().unwrap(), shape).unwrap();
        assert!(outlined(&shapes, corners), "{kind} needs a visible four-sided selection box");
    }
}

/// A job a few hundred milliseconds of rendering big.
fn big_job(doc: &Arc<figcraft_doc::Document>, page: figcraft_doc::NodeId, scale: f64) -> crate::render_worker::Job {
    crate::render_worker::Job {
        doc: doc.clone(),
        page,
        view: figcraft_render::View { origin: (0.0, 0.0), scale, width: 2400, height: 2400 },
        outline: false,
        outline_extras: Default::default(),
        outlined: vec![],
        skip: vec![],
        solo: None,
        proof: false,
        uid: 0,
        zoom_preview: false,
        time: 0.0,
        at: web_time::Instant::now(),
        texture_base: None,
        texture_view: None,
        texture_style: None,
    }
}

#[test]
fn a_job_queued_behind_a_finished_one_stays_in_flight() {
    let ctx = egui::Context::default();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    ctx.set_request_repaint_callback(move |_| {
        w.fetch_add(1, Ordering::SeqCst);
    });
    let mut s = Session::new();
    for k in 0..40 {
        s.execute("shape.ellipse", &json!({"x": k * 50, "y": k * 50, "width": 400, "height": 400})).unwrap();
    }
    let ds = s.ds().unwrap();
    let mut worker = crate::render_worker::Worker::new(ctx.clone());
    worker.submit(big_job(&ds.doc, ds.page, 1.0));
    let start = Instant::now();
    while wakes.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < Duration::from_secs(60), "the first render never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
    // As in a canvas frame: a new job goes out, then the finished one is picked up.
    worker.submit(big_job(&ds.doc, ds.page, 0.9));
    assert!(worker.poll().is_some(), "the first image");
    // Not in flight only once the second is done: its image is then waiting.
    assert!(worker.in_flight() || worker.poll().is_some(), "the second job counts until its image is there");
}

#[test]
fn the_pen_draws_the_handles_of_the_point_being_dragged_and_a_curved_band() {
    use egui::{Event, PointerButton};
    let mut u = Ui::new(Session::new());
    u.app.run("tool.set", json!({"tool": "pen"}));
    let at = |u: &Ui, x: f64, y: f64| u.app.to_screen(x, y);
    let press = |p: Pos2, pressed: bool| {
        vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() }]
    };
    let accent = egui::epaint::ColorMode::Solid(crate::theme::ACCENT);
    // A handle: the line from its anchor and the dot at its end.
    let handle = |shapes: &[Shape], a: Pos2, e: Pos2| {
        let near = |p: Pos2, q: Pos2| p.distance(q) < 0.5;
        let line = shapes.iter().any(|s| matches!(s, Shape::LineSegment { points: [p, q], .. } if near(*p, a) && near(*q, e)));
        let dot = shapes.iter().any(|s| matches!(s, Shape::Circle(c) if near(c.center, e) && c.fill == egui::Color32::WHITE));
        line && dot
    };
    let a = at(&u, 100.0, 100.0);
    u.frame(press(a, true));
    for k in 1..=4 {
        u.frame(vec![Event::PointerMoved(at(&u, 100.0 + 10.0 * f64::from(k), 100.0))]);
    }
    let shapes = flat(&u.frame_output(vec![]).shapes);
    assert!(handle(&shapes, a, at(&u, 140.0, 100.0)), "the out handle while dragging");
    assert!(handle(&shapes, a, at(&u, 60.0, 100.0)), "and the in handle opposite");
    u.frame(press(at(&u, 140.0, 100.0), false));
    // After release, towards the next point: the band curves along the out handle.
    let shapes = flat(&u.frame_output(vec![Event::PointerMoved(at(&u, 200.0, 200.0))]).shapes);
    assert!(handle(&shapes, a, at(&u, 140.0, 100.0)), "the handles stay");
    let band = shapes.iter().any(|s| matches!(s, Shape::CubicBezier(c) if c.points[0].distance(a) < 0.5 && c.points[1].distance(at(&u, 140.0, 100.0)) < 0.5 && c.stroke.color == accent));
    assert!(band, "a curve leaving along the out handle");
}

/// While drawing, the Pencil shows the stroke it will make, in its colour and weight (#256): in a
/// dark theme the theme's text colour was white, so over a white shape nothing showed until the
/// stroke was let go.
#[test]
fn the_pencil_previews_its_stroke_in_its_colour_and_weight() {
    let mut u = Ui::new(Session::new());
    u.app.run("tool.set", json!({"tool": "pencil"}));
    let (color, weight) = figcraft_engine::tools::PENCIL_STROKE;
    let preview = |shapes: &[Shape]| {
        shapes.iter().find_map(|s| match s {
            Shape::Path(p) if !p.closed && p.points.len() >= 3 && p.stroke.color == egui::epaint::ColorMode::Solid(crate::theme::c32(color)) => {
                Some(p.stroke.width)
            }
            _ => None,
        })
    };
    let at = |u: &Ui, x: f64, y: f64| u.app.to_screen(x, y);
    let start = at(&u, 10.0, 10.0);
    u.button(start, true);
    for i in 1..=6 {
        let p = at(&u, 10.0 + 12.0 * f64::from(i), 10.0 + 6.0 * f64::from(i % 2));
        u.frame(vec![egui::Event::PointerMoved(p)]);
    }
    let zoom = u.app.session.ds().unwrap().view.zoom;
    let w = preview(&flat(&u.frame_output(vec![]).shapes)).expect("a preview while drawing");
    assert!((w - (weight * zoom) as f32).abs() < 0.01, "{w} for {weight} at {zoom}");
    // Zoomed far out it stays visible.
    u.app.session.ds_mut().unwrap().view.zoom = 0.05;
    let p = at(&u, 120.0, 40.0);
    let w = preview(&flat(&u.frame_output(vec![egui::Event::PointerMoved(p)]).shapes)).expect("still previewing");
    assert!((w - 1.0).abs() < 0.01, "at least a point: {w}");
}
