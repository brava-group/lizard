//! The canvas while a drag edits the document every frame, driven through egui headless: each
//! finished render shows (a newer one already on its way doesn't hide it), and a button held still
//! stops making new documents to render.

use std::sync::Arc;
use std::time::Duration;

use egui::{Event, vec2};
use lizard_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;

/// Manual CPU/UI measurement for upstream #1571 on the current Lizard editor. Raster work has
/// already been moved to the GPU; this isolates the panel and selection overlay during a drag.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "manual large-selection UI benchmark: run with --ignored --nocapture"]
fn large_selection_ui_latency() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    let mut session = Session::new();
    let mut ids = Vec::with_capacity(648);
    for index in 0..648 {
        let value = session.execute(
            "shape.rectangle",
            &json!({
                "x": (index % 36) * 25, "y": (index / 36) * 25, "width": 20, "height": 20,
            }),
        )?;
        ids.push(lizard_engine::params::id_of(&value["id"]).ok_or("missing rectangle id")?);
    }
    session.execute("select.set", &json!({"ids": ids}))?;
    let mut ui = Ui::new(session);
    settle(&mut ui);
    let mut measure = |bypass: bool| {
        crate::selection_cache::bypass_for_test(bypass);
        for _ in 0..10 {
            ui.frame(vec![]);
        }
        let mut times = Vec::new();
        for _ in 0..80 {
            let start = Instant::now();
            ui.frame(vec![]);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        (times[40], times[76])
    };
    let original = measure(true);
    let cached = measure(false);
    crate::selection_cache::bypass_for_test(false);
    println!(
        "648 selected objects: original median {:.3} ms / p95 {:.3} ms; cached median {:.3} ms / p95 {:.3} ms",
        original.0, original.1, cached.0, cached.1
    );
    Ok(())
}

/// Frames until the render worker has nothing in flight (a few seconds at most).
fn settle(u: &mut Ui) {
    for _ in 0..300 {
        u.frame(vec![]);
        if u.app.worker.as_ref().is_some_and(|w| !w.in_flight()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_drag_shows_on_the_canvas_while_it_moves() {
    let mut s = Session::new();
    let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
    let f = id(s.execute("frame.create", &json!({"x": 100, "y": 100, "width": 300, "height": 200})).unwrap());
    s.execute("node.set", &json!({"id": f, "rotation": 25})).unwrap();
    let r =
        id(s.execute("shape.rectangle", &json!({"x": 40, "y": 40, "width": 80, "height": 60, "parent": f, "props": {"fill": "#e03030"}})).unwrap());
    s.execute("select.set", &json!({"ids": [r]})).unwrap();
    let mut u = Ui::new(s);
    settle(&mut u);
    let c = u.app.session.doc().unwrap().world_bounds(r).unwrap().center();
    let p = u.app.to_screen(c.x, c.y);
    u.button(p, true);
    settle(&mut u);
    // A move every frame, each one a new document and a new render request.
    u.app.canvas.render_work = None;
    let before = u.app.session.ds().unwrap().doc.clone();
    // At least 30 moves, then on until a finished render shows (slow machines need longer).
    for k in 1..=300 {
        u.frame(vec![Event::PointerMoved(p + vec2(k as f32, k as f32 / 3.0))]);
        std::thread::sleep(Duration::from_millis(15));
        if k >= 30 && u.app.canvas.render_work.is_some() {
            break;
        }
    }
    assert!(!Arc::ptr_eq(&before, &u.app.session.ds().unwrap().doc), "the drag moved the layer");
    assert!(u.app.canvas.render_work.is_some(), "renders finished during the drag are shown");
    assert!(!u.app.canvas.tex_coarse, "the moved layer stays at the canvas's full resolution");
    // Held still: the same move each frame keeps the preview, and the canvas settles.
    let held = u.app.session.ds().unwrap().doc.clone();
    // Frames until the last render lands (slow machines take a while): a drag held still makes
    // no new documents, so the worker runs dry.
    settle(&mut u);
    assert!(Arc::ptr_eq(&held, &u.app.session.ds().unwrap().doc));
    assert!(u.app.worker.as_ref().is_some_and(|w| !w.in_flight()), "nothing left to render");
    u.button(p + vec2(30.0, 10.0), false);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn topmost_frame_uses_composited_drag_and_returns_to_refined_vector_image() {
    let mut s = Session::new();
    let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
    let frame = id(s.execute("frame.create", &json!({"x": 100, "y": 100, "width": 420, "height": 300, "props": {"fill": "#192736"}})).unwrap());
    let child = id(s
        .execute("shape.rectangle", &json!({"x": 20, "y": 25, "width": 80, "height": 60, "parent": frame, "props": {"fill": "#e03030"}}))
        .unwrap());
    s.execute("select.set", &json!({"ids": [frame]})).unwrap();
    let mut ui = Ui::new(s);
    settle(&mut ui);
    let p = ui.app.to_screen(300.0, 200.0);
    let child_before = ui.app.session.doc().unwrap().node(child).unwrap().transform;
    ui.button(p, true);
    ui.frame(vec![Event::PointerMoved(p + vec2(140.0, 80.0))]);
    let mut composited = false;
    for _ in 0..300 {
        ui.frame(vec![]);
        if crate::canvas::drag_mode(&ui.app) == "composited" {
            composited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(composited, "the topmost frame should get a separate drag layer");
    assert_eq!(ui.app.session.doc().unwrap().node(child).unwrap().transform, child_before, "moving the frame keeps child coordinates local");
    ui.button(p + vec2(140.0, 80.0), false);
    for _ in 0..300 {
        ui.frame(vec![]);
        if ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(crate::canvas::drag_mode(&ui.app), "render");
    assert!(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)), "the final texture must replace the drag layer");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn zoomed_into_a_frame_corner_drag_stays_composited() {
    let mut s = Session::new();
    let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
    let frame = id(s.execute("frame.create", &json!({"x": 100, "y": 100, "width": 2048, "height": 1210, "props": {"fill": "#192736"}})).unwrap());
    s.execute("shape.rectangle", &json!({"parent": frame, "x": 20, "y": 30, "width": 80, "height": 60, "props": {"fill": "#e03030"}})).unwrap();
    s.execute("select.set", &json!({"ids": [frame]})).unwrap();
    let mut ui = Ui::new(s);
    settle(&mut ui);
    ui.app.canvas.refine_latency_ms = None;
    if let Ok(ds) = ui.app.session.ds_mut() {
        ds.view.zoom = 8.0;
        ds.view.cx = 130.0;
        ds.view.cy = 130.0;
        ds.revision += 1;
    }
    ui.frame(vec![]);
    for _ in 0..600 {
        ui.frame(vec![]);
        if ui.app.canvas.refine_latency_ms.is_some() && ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)));
    let start = ui.app.to_screen(115.0, 115.0);
    assert!(ui.app.canvas.rect.contains(start));
    ui.button(start, true);
    ui.frame(vec![Event::PointerMoved(start + vec2(24.0, 20.0))]);
    for _ in 0..600 {
        ui.frame(vec![]);
        if crate::canvas::drag_mode(&ui.app) == "composited" {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(crate::canvas::drag_mode(&ui.app), "composited", "high zoom should not fall back to a full vector redraw");
    ui.frame(vec![Event::PointerMoved(start + vec2(500.0, 250.0))]);
    assert_eq!(crate::canvas::drag_mode(&ui.app), "composited", "a long move should keep following the pointer");
    ui.button(start + vec2(500.0, 250.0), false);
    for _ in 0..300 {
        ui.frame(vec![]);
        if ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)), "release must refine the vector image");
}

/// A synthetic dashboard with nested cards, shapes and text. It exercises the same large-frame
/// redraw and texture conversion as a real design without depending on a proprietary file.
#[test]
#[ignore = "manual canvas latency benchmark: run with --ignored --nocapture"]
fn complex_frame_drag_latency() {
    use crate::render_worker::{Job, Renders};
    use lizard_render::View;
    use web_time::Instant;

    let mut s = Session::new();
    let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
    let root = id(s.execute("frame.create", &json!({"x": 100, "y": 80, "width": 1100, "height": 680, "props": {"fill": "#19191D"}})).unwrap());
    for row in 0..5 {
        for col in 0..7 {
            let x = 28 + col * 150;
            let y = 75 + row * 116;
            let card = id(s
                .execute("frame.create", &json!({"parent": root, "x": x, "y": y, "width": 136, "height": 102, "props": {"fill": "#28282E"}}))
                .unwrap());
            s.execute("shape.rectangle", &json!({"parent": card, "x": 14, "y": 12, "width": 34, "height": 34, "props": {"fill": "#755078"}}))
                .unwrap();
            s.execute(
                "text.create",
                &json!({"parent": card, "x": 14, "y": 58, "text": format!("Card {}", row * 7 + col), "props": {"fontSize": 14}}),
            )
            .unwrap();
            s.execute("text.create", &json!({"parent": card, "x": 14, "y": 78, "text": "A useful subtitle", "props": {"fontSize": 10}})).unwrap();
        }
    }
    let nodes = s.ds().unwrap().doc.node_count();
    let view = View { origin: (0.0, 0.0), scale: 2.0, width: 2880, height: 1800 };
    let make_job = |s: &Session| {
        let ds = s.ds().unwrap();
        Job {
            doc: ds.doc.clone(),
            page: ds.page,
            view: view.clone(),
            outline: false,
            outline_extras: Default::default(),
            outlined: vec![],
            skip: vec![],
            solo: None,
            proof: false,
            uid: ds.uid,
            zoom_preview: false,
            time: 0.0,
            at: Instant::now(),
            texture_base: None,
            texture_view: None,
            texture_style: None,
        }
    };
    let mut renders = Renders::default();
    assert!(renders.refine(&make_job(&s)).is_some());
    let mut edits = Vec::new();
    let mut paints = Vec::new();
    for x in [140, 320, 180, 620, 150, 700, 160, 360, 130, 550] {
        let t = Instant::now();
        s.execute("node.set", &json!({"id": root, "props": {"x": x}})).unwrap();
        edits.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let done = renders.refine(&make_job(&s)).unwrap();
        paints.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!done.coarse);
    }
    let median = |times: &mut Vec<f64>| {
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    println!(
        "complex dashboard: {nodes} nodes; edit median {:.1} ms; vector render + texture conversion median {:.1} ms",
        median(&mut edits),
        median(&mut paints)
    );

    s.execute("node.set", &json!({"id": root, "props": {"x": 100}})).unwrap();
    s.execute("select.set", &json!({"ids": [root]})).unwrap();
    let mut ui = Ui::new(s);
    settle(&mut ui);
    let p = ui.app.to_screen(110.0, 100.0);
    assert!(ui.app.canvas.rect.contains(p));
    ui.button(p, true);
    settle(&mut ui);
    let starting_render_ms = ui.app.ui.render_ms;
    let mut same_frame = 0;
    let mut frames = Vec::new();
    for k in 1..=10 {
        let t = web_time::Instant::now();
        ui.frame(vec![Event::PointerMoved(p + vec2(k as f32 * 8.0, 0.0))]);
        frames.push(t.elapsed().as_secs_f64() * 1000.0);
        same_frame += usize::from(ui.app.session.ds().ok().is_some_and(|ds| ui.app.canvas.texture_current(ds)));
    }
    println!(
        "canvas drag: starting render {:.1} ms; texture current in {same_frame}/10 input frames; input frame median {:.1} ms",
        starting_render_ms,
        median(&mut frames)
    );
    ui.button(p + vec2(80.0, 0.0), false);
}

/// A repeatable render measurement on the saved icon document. Set LIZARD_BENCH_DOC to the
/// .lizard file; the same page centre and device viewport are used at every zoom level.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "manual saved-document canvas benchmark: set LIZARD_BENCH_DOC and run with --ignored --nocapture"]
fn saved_document_zoom_latency() -> Result<(), Box<dyn std::error::Error>> {
    use crate::render_worker::{Job, Renders};
    use lizard_render::View;
    use web_time::Instant;

    let path = std::env::var("LIZARD_BENCH_DOC")?;
    let mut session = Session::new();
    session.execute("file.open", &json!({"path": path}))?;
    let ds = session.ds()?;
    let (width, height) = (1440_u32, 900_u32);
    let center = (ds.view.cx, ds.view.cy);
    let mut renders = Renders::default();
    let output = std::env::var("LIZARD_BENCH_OUTPUT").ok();
    println!("saved document: {} nodes; centre {center:?}; viewport {width}x{height}", ds.doc.node_count());
    for scale in [1.0, 8.0, 16.0, 8.0, 1.0] {
        let view = View { origin: (center.0 - f64::from(width) / (2.0 * scale), center.1 - f64::from(height) / (2.0 * scale)), scale, width, height };
        let job = Job {
            doc: ds.doc.clone(),
            page: ds.page,
            view,
            time: 0.0,
            outline: false,
            outline_extras: Default::default(),
            outlined: Vec::new(),
            skip: Vec::new(),
            solo: None,
            proof: false,
            uid: ds.uid,
            zoom_preview: scale >= 4.0,
            at: Instant::now(),
            texture_base: None,
            texture_view: None,
            texture_style: None,
        };
        let started = Instant::now();
        if let Some(preview) = renders.preview(&job) {
            println!(
                "scale {scale:>4.0}x: sharp geometry preview {:.1} ms, {}x{} device pixels",
                started.elapsed().as_secs_f64() * 1000.0,
                preview.view.width,
                preview.view.height
            );
            if scale == 16.0
                && let Some(path) = &output
            {
                let rgba: Vec<u8> = preview.image.pixels.iter().flat_map(|pixel| pixel.to_array()).collect();
                image::save_buffer(path, &rgba, preview.image.size[0] as u32, preview.image.size[1] as u32, image::ColorType::Rgba8)?;
            }
        }
        let done = renders.refine(&job).ok_or("render failed")?;
        println!("scale {scale:>4.0}x: render {:.1} ms, total {:.1} ms, work {:?}", done.ms, started.elapsed().as_secs_f64() * 1000.0, done.work);
    }
    Ok(())
}

#[test]
fn releasing_a_canvas_drag_outside_finishes_the_interaction() {
    let mut s = Session::new();
    let r =
        lizard_engine::params::id_of(&s.execute("shape.rectangle", &json!({"x": 40, "y": 40, "width": 80, "height": 60})).unwrap()["id"]).unwrap();
    s.execute("select.set", &json!({"ids": [r]})).unwrap();
    let mut u = Ui::new(s);
    let start = u.app.to_screen(80.0, 70.0);
    let outside = egui::pos2(u.app.canvas.rect.max.x + 20.0, start.y);
    u.button(start, true);
    u.frame(vec![Event::PointerMoved(outside)]);
    u.button(outside, false);

    assert!(u.app.session.ds().unwrap().interaction.is_none(), "the document interaction is committed");
}

fn key(u: &mut Ui, key: egui::Key) {
    let modifiers = egui::Modifiers::NONE;
    u.frame(vec![
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
    ]);
}

#[test]
fn a_drag_released_off_the_window_ends_where_the_pointer_was_last_seen_and_later_edits_stay() {
    // #278: a drag left open made a later text edit roll back the changes made since.
    let mut s = Session::new();
    let id = |v: serde_json::Value| lizard_engine::params::id_of(&v["id"]).unwrap();
    let kept = id(s.execute("shape.rectangle", &json!({"x": 40, "y": 400, "width": 80, "height": 40})).unwrap());
    let gone = id(s.execute("shape.rectangle", &json!({"x": 160, "y": 400, "width": 80, "height": 40})).unwrap());
    let text = id(s.execute("text.create", &json!({"text": "A", "x": 40, "y": 500})).unwrap());
    s.execute("tool.rectangle", &json!({})).unwrap();
    let mut u = Ui::new(s);
    // A rectangle drawn from the middle of the canvas, released off the window, the pointer gone.
    let start = u.app.canvas.rect.center();
    u.button(start, true);
    for i in 1..=6 {
        u.frame(vec![Event::PointerMoved(start + vec2(10.0, 6.0) * i as f32)]);
    }
    let release =
        Event::PointerButton { pos: egui::pos2(-50.0, -50.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() };
    u.frame(vec![release, Event::PointerGone]);
    let ds = u.app.session.ds().unwrap();
    assert!(ds.interaction.is_none(), "the drawing is committed");
    let (drawn, zoom) = (ds.selection[0], ds.view.zoom);
    let b = ds.doc.world_bounds(drawn).unwrap();
    // Not the 100 x 100 shape of a click, as an end in the middle of the canvas would make.
    assert!((b.width() * zoom - 60.0).abs() < 1.5 && (b.height() * zoom - 36.0).abs() < 1.5, "{b:?} at zoom {zoom}");
    // #278's steps: a move in the panel, a delete, then a text edit.
    u.app.run("node.set", json!({"ids": [kept], "props": {"x": 650}}));
    u.app.run("edit.delete", json!({"ids": [gone]}));
    u.app.run("select.set", json!({"ids": [text]}));
    key(&mut u, egui::Key::Enter);
    u.frame(vec![Event::Text("B".into())]);
    key(&mut u, egui::Key::Escape);
    let doc = u.app.session.doc().unwrap();
    assert_eq!(doc.world_bounds(kept).unwrap().x0, 650.0, "the move stays");
    assert!(!doc.contains(gone), "the delete stays");
    assert_eq!(doc.node(text).unwrap().kind.text().unwrap().characters, "B");
}
