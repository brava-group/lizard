//! Coarse previews while the zoom changes (#295): a new zoom first shows the whole view coarsely,
//! then the refined image, which matches a render from scratch; pans and edits still build on the
//! kept image; previews never replace a refined image that already shows the whole view asked
//! for, and no image of another file or page goes on screen. The browser's inline worker previews
//! by the same measure as the render thread.

use std::sync::Arc;

use figcraft_doc::{Document, NodeId};
use figcraft_engine::Session;
use figcraft_engine::params::id_of;
use figcraft_render::{View, Work};
use serde_json::json;

use super::{CanvasState, canvas_zoom_delta, canvas_zoom_events, measured_wheel_zoom, shows};
use crate::render_worker::{Done, Inline, Job, PREVIEW_AFTER_MS, PREVIEW_FACTOR, Renders, TextureUpdate, coarse_view, preview_now, render_now};

/// A page of ellipses with a rectangle on top; the session and the rectangle.
fn document() -> (Session, NodeId) {
    let mut s = Session::new();
    for k in 0..12 {
        let k = f64::from(k);
        s.execute("shape.ellipse", &json!({"x": k * 60.0, "y": k * 40.0, "width": 120, "height": 90})).unwrap();
    }
    let made = s.execute("shape.rectangle", &json!({"x": 100, "y": 50, "width": 300, "height": 200})).unwrap();
    let rect = id_of(&made["id"]).unwrap();
    (s, rect)
}

/// A page whose renders from scratch of each of `views` take at least `ms`: the rectangle with glass
/// and a background blur, and more and more blurred rectangles over the views until they do (a
/// fixed scene would turn quick on a faster machine or renderer).
fn slow_scene(views: &[View], ms: f64) -> Session {
    let (mut s, rect) = document();
    s.execute("effect.add", &json!({"ids": [rect], "type": "GLASS"})).unwrap();
    s.execute("effect.add", &json!({"ids": [rect], "type": "BACKGROUND_BLUR"})).unwrap();
    let area = views.iter().map(View::page_rect).reduce(|a, b| a.union(b)).unwrap();
    let mut more = 8;
    for _ in 0..12 {
        let ds = s.ds().unwrap();
        let quickest = views.iter().map(|v| render_now(&job(&ds.doc, ds.page, ds.uid, v.clone())).unwrap().ms).fold(f64::INFINITY, f64::min);
        if quickest >= ms {
            return s;
        }
        let (w, h) = (area.width() / 4.0, area.height() / 4.0);
        let ids: Vec<NodeId> = (0..more)
            .map(|k| {
                let (fx, fy) = (f64::from(k * 37 % 101) / 101.0, f64::from(k * 59 % 103) / 103.0);
                let p = json!({"x": area.x0 + fx * (area.width() - w), "y": area.y0 + fy * (area.height() - h), "width": w, "height": h});
                id_of(&s.execute("shape.rectangle", &p).unwrap()["id"]).unwrap()
            })
            .collect();
        s.execute("effect.add", &json!({"ids": ids, "type": "BACKGROUND_BLUR"})).unwrap();
        more *= 2;
    }
    panic!("no scene renders slower than {ms} ms");
}

/// A `w` × `h` device-pixel view at `scale` centred on page point `c`.
fn view_at(c: (f64, f64), scale: f64, w: u32, h: u32) -> View {
    View { origin: (c.0 - f64::from(w) / scale / 2.0, c.1 - f64::from(h) / scale / 2.0), scale, width: w, height: h }
}

fn job(doc: &Arc<Document>, page: NodeId, uid: u64, view: View) -> Job {
    Job {
        doc: doc.clone(),
        page,
        view,
        outline: false,
        outline_extras: Default::default(),
        outlined: vec![],
        skip: vec![],
        solo: None,
        proof: false,
        uid,
        zoom_preview: true,
        time: 0.0,
        at: web_time::Instant::now(),
        texture_base: None,
        texture_view: None,
        texture_style: None,
    }
}

/// Every pixel of `a` and `b` alike.
fn same_pixels(a: &Done, b: &Done) -> bool {
    a.image.size == b.image.size && a.image.pixels == b.image.pixels
}

#[test]
fn revisiting_a_zoom_reuses_exact_pixels_and_an_edit_invalidates_it() {
    let (mut session, rect) = document();
    let ds = session.ds().unwrap();
    let low = job(&ds.doc, ds.page, ds.uid, view_at((180.0, 120.0), 1.0, 320, 200));
    let high = job(&ds.doc, ds.page, ds.uid, view_at((180.0, 120.0), 2.0, 320, 200));
    let mut renders = Renders::with_threshold(0.0);
    assert_eq!(renders.refine(&low).unwrap().work, Work::Full);
    assert_eq!(renders.refine(&high).unwrap().work, Work::Full);
    assert!(renders.preview(&low).is_none(), "a cached full zoom should skip the coarse preview");
    let returned = renders.refine(&low).unwrap();
    assert_eq!(returned.work, Work::Reused);
    assert!(same_pixels(&returned, &render_now(&low).unwrap()));

    session.execute("node.set", &json!({"id": rect, "props": {"x": 125}})).unwrap();
    let edited = session.ds().unwrap();
    let changed = job(&edited.doc, edited.page, edited.uid, low.view.clone());
    let updated = renders.refine(&changed).unwrap();
    assert!(same_pixels(&updated, &render_now(&changed).unwrap()));
}

#[test]
fn continuous_zoom_moves_less_per_gesture_and_is_symmetric() {
    let zoom_in = canvas_zoom_delta(1.25).unwrap();
    let zoom_out = canvas_zoom_delta(0.8).unwrap();
    assert!((1.0..1.25).contains(&zoom_in));
    assert!((0.8..1.0).contains(&zoom_out));
    assert!((zoom_in * zoom_out - 1.0).abs() < 1e-6);
    assert_eq!(canvas_zoom_delta(1.0), Some(1.0));
    assert_eq!(canvas_zoom_delta(0.0), None);
    assert_eq!(canvas_zoom_delta(f32::NAN), None);
}

#[test]
fn measured_pixel_wheel_zoom_matches_browser_fixtures() {
    let meta = egui::Modifiers { mac_cmd: true, ..Default::default() };
    let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
    for (delta, percent) in [(1.0, 101.0), (5.0, 103.0), (10.0, 105.0), (20.0, 111.0), (30.0, 116.0), (60.0, 121.0), (120.0, 127.0), (240.0, 135.0)] {
        let factor = measured_wheel_zoom(delta, meta).unwrap();
        assert_eq!((factor * 100.0).round(), percent, "Meta wheel delta {delta}");
        let inverse = measured_wheel_zoom(-delta, meta).unwrap();
        assert!((factor * inverse - 1.0).abs() < 1e-10);
    }
    for (delta, percent) in [(1.0, 101.0), (5.0, 107.0), (10.0, 115.0), (20.0, 132.0), (30.0, 152.0), (60.0, 230.0), (120.0, 528.0)] {
        let factor = measured_wheel_zoom(delta, ctrl).unwrap();
        assert_eq!((factor * 100.0).round(), percent, "Ctrl wheel delta {delta}");
    }
    assert_eq!(measured_wheel_zoom(120.0, egui::Modifiers::default()), None);
    assert_eq!(measured_wheel_zoom(f32::NAN, ctrl), None);
    assert_eq!(measured_wheel_zoom(100_000.0, ctrl), Some(10.0));
}

#[test]
fn explicit_pinch_zoom_is_not_doubled_by_egui_wheel_smoothing() {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Zoom(1.25));
    ctx.begin_pass(raw);
    let factors = ctx.input(canvas_zoom_events);
    ctx.end_pass().textures_delta.clear();
    assert_eq!(factors, vec![canvas_zoom_delta(1.25).unwrap()]);
    ctx.begin_pass(egui::RawInput::default());
    assert!(ctx.input(canvas_zoom_events).is_empty());
    ctx.end_pass().textures_delta.clear();
}

#[test]
fn pixel_wheel_zoom_is_processed_once_per_raw_event() {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 120.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers { ctrl: true, ..Default::default() },
    });
    ctx.begin_pass(raw);
    let factors = ctx.input(canvas_zoom_events);
    assert_eq!(factors.len(), 1);
    if cfg!(any(target_os = "macos", target_arch = "wasm32")) {
        assert!((factors[0] - 4.0_f64.powf(1.2)).abs() < 1e-9);
    }
    ctx.end_pass().textures_delta.clear();
    ctx.begin_pass(egui::RawInput::default());
    assert!(ctx.input(canvas_zoom_events).is_empty(), "egui may still be smoothing, but the raw wheel event is gone");
    ctx.end_pass().textures_delta.clear();
}

#[test]
fn zoom_out_keeps_input_responsive_until_the_worker_covers_the_new_view() {
    let (s, _) = document();
    let mut ui = crate::layers_drag_tests::Ui::new(s);
    ui.app.run("view.zoom200", json!({}));
    // Parallel workspace tests can delay the render worker even when the canvas is responsive.
    for _ in 0..400 {
        ui.frame(vec![]);
        if ui.app.worker.as_ref().is_some_and(|w| !w.in_flight()) && !ui.app.canvas.tex_coarse {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let old = ui.app.canvas.tex_view.clone().expect("the starting vector image");
    ui.app.run("view.zoom50", json!({}));
    ui.frame(vec![]);
    let current = ui.app.canvas.tex_view.clone().expect("the previous image stays available");
    let ds = ui.app.session.ds().unwrap();
    let (w, h) = (ui.app.canvas.rect.width() as u32, ui.app.canvas.rect.height() as u32);
    let visible = View { origin: ds.view.origin(), scale: ds.view.zoom, width: w, height: h };
    assert!(!covers(&old, &visible), "the old image cannot cover the zoomed-out viewport");
    assert!(!ui.app.canvas.tex_coarse, "zoom must not synchronously rasterize a coarse replacement");
    assert!(
        (current.origin == old.origin && current.scale == old.scale && current.width == old.width && current.height == old.height)
            || covers(&current, &visible),
        "input uses cached pixels or an already completed worker image"
    );
    for _ in 0..400 {
        ui.frame(vec![]);
        if ui.app.canvas.tex_view.as_ref().is_some_and(|view| covers(view, &visible)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the worker did not fill the zoomed-out viewport");
}

#[test]
fn immediate_preview_keeps_native_resolution() {
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let j = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 1.0, 800, 600));
    let immediate = preview_now(&j).unwrap();
    let full = render_now(&j).unwrap();
    assert!(immediate.coarse && same_pixels(&immediate, &full));
}

#[test]
fn a_retained_wide_view_covers_zoom_out_without_synchronous_rasterization() {
    let (s, _) = document();
    let mut ui = crate::layers_drag_tests::Ui::new(s);
    let settle = |ui: &mut crate::layers_drag_tests::Ui| {
        for _ in 0..150 {
            ui.frame(vec![]);
            let requested_is_refined = ui
                .app
                .canvas
                .tex_view
                .as_ref()
                .zip(ui.app.canvas.want_view.as_ref())
                .is_some_and(|(have, want)| have.scale.to_bits() == want.scale.to_bits() && covers(have, want));
            if requested_is_refined && ui.app.worker.as_ref().is_some_and(|w| !w.in_flight()) && !ui.app.canvas.tex_coarse {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("vector render did not finish");
    };
    ui.app.run("view.zoom50", json!({}));
    settle(&mut ui);
    ui.app.run("view.zoom200", json!({}));
    settle(&mut ui);
    assert!(ui.app.canvas.zoom_fallback.is_some(), "the wide vector image remains available");

    ui.app.run("view.zoom100", json!({}));
    ui.frame(vec![]);
    assert!(!ui.app.canvas.tex_coarse, "the input frame reuses existing pixels instead of rendering a preview");
    let ds = ui.app.session.ds().unwrap();
    assert_eq!(ui.app.canvas.uncovered_pixels(ds), Some(0), "the retained view covers the exposed margins");
}

#[test]
fn a_drag_converts_only_damage_when_the_visible_texture_is_its_base() {
    let (mut s, rect) = document();
    let ds = s.ds().unwrap();
    let view = view_at((300.0, 200.0), 1.0, 800, 600);
    let mut renders = Renders::default();
    let first = renders.refine(&job(&ds.doc, ds.page, ds.uid, view.clone())).unwrap();
    assert!(matches!(first.update, TextureUpdate::Full(_)));

    s.execute("node.set", &json!({"id": rect, "props": {"x": 120}})).unwrap();
    let ds = s.ds().unwrap();
    let mut moved = job(&ds.doc, ds.page, ds.uid, view.clone());
    moved.texture_base = Some(first.doc.clone());
    moved.texture_view = Some(first.view.clone());
    moved.texture_style = Some(first.style.clone());
    let second = renders.refine(&moved).unwrap();
    let TextureUpdate::Partial(patches) = &second.update else { panic!("the drag should upload local pixels") };
    let uploaded: usize = patches.iter().map(|(_, image)| image.size[0] * image.size[1]).sum();
    assert!(uploaded < second.image.size[0] * second.image.size[1] / 2);
    for (at, patch) in patches {
        for row in 0..patch.size[1] {
            let start = (at[1] + row) * second.image.size[0] + at[0];
            let part = row * patch.size[0];
            assert_eq!(&patch.pixels[part..part + patch.size[0]], &second.image.pixels[start..start + patch.size[0]]);
        }
    }

    s.execute("node.set", &json!({"id": rect, "props": {"x": 140}})).unwrap();
    let ds = s.ds().unwrap();
    let mut skipped = job(&ds.doc, ds.page, ds.uid, view);
    skipped.texture_base = Some(first.doc);
    skipped.texture_view = Some(first.view);
    skipped.texture_style = Some(first.style);
    let third = renders.refine(&skipped).unwrap();
    let TextureUpdate::Partial(patches) = &third.update else { panic!("the skipped positions should upload damage against the displayed document") };
    assert_eq!(third.base_docs.len(), 2);
    for mut displayed in [first.image, second.image] {
        for (at, patch) in patches {
            for row in 0..patch.size[1] {
                let start = (at[1] + row) * displayed.size[0] + at[0];
                let part = row * patch.size[0];
                displayed.pixels[start..start + patch.size[0]].copy_from_slice(&patch.pixels[part..part + patch.size[0]]);
            }
        }
        assert_eq!(displayed.pixels, third.image.pixels);
    }
}

fn covers(outer: &View, inner: &View) -> bool {
    let (o, i) = (outer.page_rect(), inner.page_rect());
    let eps = 1e-6;
    o.x0 <= i.x0 + eps && o.y0 <= i.y0 + eps && o.x1 >= i.x1 - eps && o.y1 >= i.y1 - eps
}

#[test]
fn a_zoom_out_previews_the_whole_view_then_refines_to_a_fresh_render() {
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let mut r = Renders::with_threshold(0.0);
    let zoomed_in = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 8.0, 400, 300));
    assert!(r.preview(&zoomed_in).is_none(), "the first render has nothing to measure previews by");
    assert!(r.refine(&zoomed_in).is_some());
    // 800% → 100% about the same centre: the old image covers 1/64 of the new view.
    let out = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 1.0, 400, 300));
    let p = r.preview(&out).expect("a preview first");
    assert!(p.coarse && (p.uid, p.page) == (ds.uid, ds.page));
    assert!(covers(&p.view, &out.view), "the preview shows the whole view asked for");
    let pixels = |d: &Done| d.image.size[0] * d.image.size[1];
    let fresh = render_now(&out).unwrap();
    assert!(
        (pixels(&p) as f64) * PREVIEW_FACTOR * PREVIEW_FACTOR < 1.5 * pixels(&fresh) as f64,
        "a quarter of the pixels, give or take the snapping"
    );
    let refined = r.refine(&out).unwrap();
    assert!(!refined.coarse && refined.work == Work::Full);
    assert!(same_pixels(&refined, &fresh), "the settled image is a render from scratch");
}

#[test]
fn a_small_zoom_keeps_the_visible_full_resolution_texture_until_refined() {
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let mut renders = Renders::with_threshold(0.0);
    let start = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 1.0, 800, 600));
    let old = renders.refine(&start).unwrap();
    let mut near = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 1.1, 800, 600));
    near.texture_base = Some(old.doc.clone());
    near.texture_view = Some(old.view.clone());
    near.texture_style = Some(old.style.clone());
    assert!(renders.preview(&near).is_none(), "a slight pinch keeps sharper pixels already on screen");
    let mut edited = near.clone();
    edited.doc = Arc::new((*old.doc).clone());
    assert!(renders.preview(&edited).is_some(), "stale content cannot suppress the new preview");
    let mut changed_style = near.clone();
    changed_style.proof = true;
    assert!(renders.preview(&changed_style).is_some(), "a style change needs the new preview");
    let refined = renders.refine(&near).unwrap();
    assert!(!refined.coarse && same_pixels(&refined, &render_now(&near).unwrap()));

    let mut far = near;
    far.view = view_at((300.0, 200.0), 2.1, 800, 600);
    far.texture_view = Some(old.view);
    assert!(renders.preview(&far).is_some(), "a large zoom gets a fresh preview while it renders");
}

/// Zoom, pan and edit with `r`; whether each step was previewed, and the pan's and the edit's
/// images.
fn zoom_pan_edit(r: &mut Renders) -> (Vec<bool>, Done, Done) {
    let (mut s, rect) = document();
    let (doc, page, uid) = {
        let ds = s.ds().unwrap();
        (ds.doc.clone(), ds.page, ds.uid)
    };
    r.refine(&job(&doc, page, uid, view_at((300.0, 200.0), 4.0, 1280, 960))).unwrap();
    let mut previewed = vec![];
    // A zoom; the preview is kept apart from the full image.
    let zoomed = job(&doc, page, uid, view_at((300.0, 200.0), 2.0, 1280, 960));
    previewed.push(r.preview(&zoomed).is_some());
    r.refine(&zoomed).unwrap();
    // A pan at that zoom.
    let mut pan = zoomed;
    pan.view.origin.0 += 10.0;
    previewed.push(r.preview(&pan).is_some());
    let panned = r.refine(&pan).unwrap();
    // An edit there.
    s.execute("node.set", &json!({"id": rect, "props": {"x": 120}})).unwrap();
    let edit = job(&s.ds().unwrap().doc, page, uid, pan.view);
    previewed.push(r.preview(&edit).is_some());
    let edited = r.refine(&edit).unwrap();
    (previewed, panned, edited)
}

#[test]
fn pans_and_edits_get_no_preview_and_build_on_the_kept_image() {
    let (previewed, panned, edited) = zoom_pan_edit(&mut Renders::with_threshold(0.0));
    assert_eq!(previewed, [true, false, false], "the zoom only");
    assert!(matches!(panned.work, Work::Partial { .. }), "a pan after a previewed zoom renders strips only: {:?}", panned.work);
    assert!(matches!(edited.work, Work::Partial { .. }), "an edit renders its area only: {:?}", edited.work);
    // The same steps without previews: the same work and the same pixels.
    let (none, panned_alone, edited_alone) = zoom_pan_edit(&mut Renders::with_threshold(f64::INFINITY));
    assert_eq!(none, [false, false, false]);
    assert_eq!((panned.work, edited.work), (panned_alone.work, edited_alone.work));
    assert!(same_pixels(&panned, &panned_alone) && same_pixels(&edited, &edited_alone));
}

#[test]
fn edited_layer_renders_at_full_resolution_without_a_drag_preview() {
    let (mut s, rect) = document();
    let view = view_at((300.0, 200.0), 2.0, 800, 600);
    let mut renders = Renders::with_threshold(0.0);
    let ds = s.ds().unwrap();
    let initial = job(&ds.doc, ds.page, ds.uid, view.clone());
    renders.refine(&initial).unwrap();
    let mut without_preview = Renders::with_threshold(f64::INFINITY);
    without_preview.refine(&initial).unwrap();

    s.execute("node.set", &json!({"id": rect, "props": {"x": 120}})).unwrap();
    let ds = s.ds().unwrap();
    let moving = job(&ds.doc, ds.page, ds.uid, view);
    assert!(renders.preview(&moving).is_none(), "moving a layer at the same zoom does not reduce resolution");
    let refined = renders.refine(&moving).unwrap();
    assert!(!refined.coarse);
    assert!(same_pixels(&refined, &without_preview.refine(&moving).unwrap()));
}

#[test]
fn previews_come_for_other_pages_and_files_but_not_in_pixel_preview_or_when_renders_are_quick() {
    let (mut s, _) = document();
    let (doc, page, uid) = {
        let ds = s.ds().unwrap();
        (ds.doc.clone(), ds.page, ds.uid)
    };
    let v = view_at((300.0, 200.0), 1.0, 200, 150);
    let mut r = Renders::with_threshold(0.0);
    r.refine(&job(&doc, page, uid, v.clone())).unwrap();
    s.execute("page.add", &json!({})).unwrap();
    let ds = s.ds().unwrap();
    assert!(r.preview(&job(&ds.doc, ds.page, uid, v.clone())).is_some(), "another page");
    assert!(r.preview(&job(&doc, page, uid + 1, v.clone())).is_some(), "another file");
    let zoom = view_at((300.0, 200.0), 2.0, 200, 150);
    let mut pixel_preview = job(&doc, page, uid, zoom.clone());
    pixel_preview.zoom_preview = false;
    assert!(r.preview(&pixel_preview).is_none(), "pixel preview shows real pixels only");
    let mut quick = Renders::with_threshold(f64::INFINITY);
    quick.refine(&job(&doc, page, uid, v)).unwrap();
    assert!(quick.preview(&job(&doc, page, uid, zoom)).is_none(), "quick renders show without one");
}

#[test]
fn previews_cover_the_view_for_any_zoom_and_density() {
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let mut r = Renders::with_threshold(0.0);
    r.refine(&job(&ds.doc, ds.page, ds.uid, view_at((0.0, 0.0), 0.37, 64, 64))).unwrap();
    let mut n = 0;
    // Zooms from 2% to 6400% at 1x, 1.5x and 2x displays, odd sizes, centres off the pixel grid.
    for zoom in [0.02, 0.1, 0.333, 1.0, 2.5, 8.0, 64.0] {
        for ppp in [1.0, 1.5, 2.0] {
            for (w, h) in [(1, 1), (3, 7), (401, 299), (1280, 720)] {
                let v = view_at((123.456, -78.9), zoom * ppp, w, h);
                let c = coarse_view(&v);
                assert!(covers(&c, &v), "coarse view of {v:?}");
                let p = r.preview(&job(&ds.doc, ds.page, ds.uid, v.clone())).expect("a preview");
                assert!(covers(&p.view, &v), "preview of {v:?} shows {:?}", p.view);
                n += 1;
            }
        }
    }
    assert_eq!(n, 84);
}

#[test]
fn outline_mode_proofing_and_glass_preview_and_settle_like_fresh_renders() {
    let (mut s, rect) = document();
    s.execute("effect.add", &json!({"ids": [rect], "type": "GLASS"})).unwrap();
    let ds = s.ds().unwrap();
    for (outline, proof) in [(false, false), (true, false), (false, true)] {
        let mut r = Renders::with_threshold(0.0);
        let mut first = job(&ds.doc, ds.page, ds.uid, view_at((250.0, 150.0), 3.0, 300, 200));
        (first.outline, first.proof) = (outline, proof);
        r.refine(&first).unwrap();
        let mut next = first.clone();
        next.view = view_at((250.0, 150.0), 0.75, 300, 200);
        let p = r.preview(&next).expect("a preview");
        assert!(p.coarse && covers(&p.view, &next.view));
        let d = r.refine(&next).unwrap();
        assert!(same_pixels(&d, &render_now(&next).unwrap()), "outline {outline}, proof {proof}");
    }
}

#[test]
fn effect_heavy_zoom_first_shows_native_resolution_geometry() {
    let (mut session, rect) = document();
    session.execute("effect.add", &json!({"ids": [rect], "type": "DROP_SHADOW"})).unwrap();
    let ds = session.ds().unwrap();
    let view = view_at((100.0, 100.0), 8.0, 320, 240);
    let job = job(&ds.doc, ds.page, ds.uid, view.clone());
    let mut renders = Renders::with_threshold(f64::INFINITY);
    let preview = renders.preview(&job).expect("high zoom should show geometry before effects");
    assert!(preview.coarse);
    assert_eq!(preview.view.scale, view.scale);
    assert!(covers(&preview.view, &view));
    let full = renders.refine(&job).expect("full effect render");
    assert!(same_pixels(&full, &render_now(&job).unwrap()));
    assert!(!same_pixels(&preview, &full), "the shadow is composed only in the final pass");
}

#[test]
fn the_canvas_shows_images_of_its_own_page_and_never_a_coarser_one_of_its_view() {
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let here = Some((ds.uid, ds.page));
    let mut r = Renders::with_threshold(0.0);
    let near = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 2.0, 200, 150));
    let refined = r.refine(&near).unwrap();
    let far = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 0.5, 200, 150));
    let preview = r.preview(&far).unwrap();

    let mut c = CanvasState { want_view: Some(near.view.clone()), ..Default::default() };
    assert!(shows(&c, &refined, here));
    assert!(!shows(&c, &refined, Some((ds.uid + 1, ds.page))), "another file");
    assert!(!shows(&c, &refined, Some((ds.uid, NodeId(ds.page.0 + 1)))), "another page");
    assert!(!shows(&c, &refined, None), "no file open");
    // Showing the refined image of the view asked for: a preview arriving late stays off.
    (c.tex_view, c.tex_of, c.tex_doc, c.tex_coarse) = (Some(refined.view.clone()), here, Some(refined.doc.clone()), false);
    assert!(!shows(&c, &preview, here));
    // Once the view asked for moves on, the preview is the better match.
    c.want_view = Some(far.view.clone());
    assert!(shows(&c, &preview, here));
    // Over another preview, or a refined image of another page, it goes on.
    c.want_view = Some(near.view.clone());
    c.tex_coarse = true;
    assert!(shows(&c, &preview, here));
    (c.tex_coarse, c.tex_of) = (false, Some((ds.uid, NodeId(ds.page.0 + 1))));
    assert!(shows(&c, &preview, here));
}

#[test]
fn an_old_zoom_render_cannot_replace_a_preview_that_covers_the_new_view() {
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let here = Some((ds.uid, ds.page));
    let mut renders = Renders::with_threshold(0.0);
    let old = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 2.0, 200, 150));
    let new = job(&ds.doc, ds.page, ds.uid, view_at((300.0, 200.0), 0.5, 200, 150));
    let previous_full = renders.refine(&old).unwrap();
    let new_preview = renders.preview(&new).unwrap();
    assert!(covers(&new_preview.view, &new.view));
    assert!(!covers(&previous_full.view, &new.view));
    let canvas = CanvasState {
        tex_view: Some(new_preview.view.clone()),
        tex_of: here,
        tex_doc: Some(new_preview.doc.clone()),
        tex_coarse: true,
        want_view: Some(new.view.clone()),
        ..Default::default()
    };
    assert!(!shows(&canvas, &previous_full, here));
    assert!(shows(&canvas, &renders.refine(&new).unwrap(), here));
}

#[test]
fn a_preview_replaces_a_refined_image_of_the_same_zoom_elsewhere_on_the_page() {
    // 800 × 600: A at 100% from the page origin, B at 800% about its top-left, A' back at 100%
    // about B's bottom-right corner. A shows 100 × 75 of A', 1/64 of it.
    let (s, _) = document();
    let ds = s.ds().unwrap();
    let here = Some((ds.uid, ds.page));
    let view = |origin, scale| View { origin, scale, width: 800, height: 600 };
    let a = job(&ds.doc, ds.page, ds.uid, view((0.0, 0.0), 1.0));
    let b = job(&ds.doc, ds.page, ds.uid, view((0.0, 0.0), 8.0));
    let a2 = job(&ds.doc, ds.page, ds.uid, view((-700.0, -525.0), 1.0));
    let mut r = Renders::with_threshold(0.0);
    let refined_a = r.refine(&a).unwrap();
    // The canvas shows A; B is asked for, then A' while B renders. The thread finishes B, then
    // previews A' before the canvas polls: the queue holds [refined B, preview A'], and a poll
    // takes the last.
    let queue = [r.refine(&b).unwrap(), r.preview(&a2).expect("A' is a new zoom for the thread")];
    let polled = queue.last().unwrap();
    assert!(polled.coarse && covers(&polled.view, &a2.view));
    let c = CanvasState {
        tex_view: Some(refined_a.view.clone()),
        tex_of: here,
        tex_doc: Some(refined_a.doc.clone()),
        tex_coarse: false,
        want_view: Some(a2.view.clone()),
        ..Default::default()
    };
    assert!(shows(&c, polled, here), "A at the same zoom shows 1/64 of A': the preview of all of it goes on");
    // Once A' itself is refined on screen, a preview of it would be a step down.
    let refined_a2 = r.refine(&a2).unwrap();
    let c = CanvasState { tex_view: Some(refined_a2.view.clone()), ..c };
    assert!(!shows(&c, polled, here));
}

#[test]
fn the_browser_worker_previews_by_measured_render_time_then_refines_the_next_frame() {
    // Renders from scratch at 200% and 100% (whose times decide the previews below) well over the
    // threshold, wherever this runs.
    let view = |scale| view_at((300.0, 200.0), scale, 1200, 1200);
    let s = slow_scene(&[view(2.0), view(1.0)], 3.0 * PREVIEW_AFTER_MS);
    let ds = s.ds().unwrap();
    let at = |scale| job(&ds.doc, ds.page, ds.uid, view(scale));
    // The production threshold, as the browser has it.
    let mut w = Inline::new(egui::Context::default());
    w.submit(at(2.0));
    let first = w.poll().expect("the first image");
    assert!(!first.coarse && !w.in_flight(), "the first render has no measure to preview by");
    assert!(first.ms >= PREVIEW_AFTER_MS, "renders are timed in the browser worker too (and this one is slow): {} ms", first.ms);
    // A new zoom: the preview this frame, the refined image the next.
    w.submit(at(1.0));
    let p = w.poll().expect("a preview");
    assert!(p.coarse && w.in_flight());
    let d = w.poll().expect("the refined image");
    assert!(!d.coarse && !w.in_flight());
    assert!(same_pixels(&d, &render_now(&at(1.0)).unwrap()));
    // A newer request before the refinement: refining the old one is skipped.
    w.submit(at(0.5));
    assert!(w.poll().is_some_and(|d| d.coarse));
    w.submit(at(0.25));
    let p = w.poll().expect("the newer one's preview");
    assert!(p.coarse && (p.view.scale - 0.25 / PREVIEW_FACTOR).abs() < 1e-12);
    assert!(w.poll().is_some_and(|d| !d.coarse && d.view.scale == 0.25));
    assert!(w.poll().is_none() && !w.in_flight());
}
