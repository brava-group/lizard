//! Plugin API 4 in the UI, driven headless: a widget's text is typed in place on the canvas and
//! committed to its plugin; the property menu sits by the selected widget and in the Design panel,
//! named for screen readers and reachable from the keyboard; a part's hover style and tooltip show
//! without a callback; a pending timer wakes the window when it is due and at no other time; and a
//! panel's slider tells its plugin now and then while dragged (a presented prototype's side is in
//! `present_widgets_tests`). The test plugin is WAT: it draws what its input asks.

use std::time::Duration;

use base64::Engine as _;
use egui::accesskit::{Node, Role};
use egui::{Event, Key, Modifiers, Pos2, Rect};
use figcraft_doc::{Document, NodeId};
use figcraft_engine::Session;
use serde_json::{Value, json};

use crate::a11y::Region;
use crate::idle_tests::Harness;
use crate::layers_drag_tests::Ui;

pub(crate) const NOTE: &str = "org.test.note";

/// A module that, for the first of `cases` whose needle its input contains (an empty needle
/// always does), draws the look (unless it is null) and asks for a timer (unless it is negative).
fn module(imports: &str, cases: &[(&str, Value, i32)], call: &str) -> Vec<u8> {
    let lit = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    let (mut data, mut at, mut body) = (String::new(), 1000usize, String::new());
    let mut put = |text: &str| {
        let a = at;
        data += &format!("(data (i32.const {a}) \"{}\")\n", lit(text));
        at += text.len();
        (a, text.len())
    };
    for (needle, look, timer) in cases {
        let ((n, nl), (l, ll)) = (put(needle), put(&look.to_string()));
        let draw = if look.is_null() { String::new() } else { format!("(drop (call $draw (i32.const {l}) (i32.const {ll})))") };
        let timer = if *timer < 0 { String::new() } else { format!("(drop (call $timer (i32.const {timer})))") };
        body += &format!("(if (call $has (i32.const {n}) (i32.const {nl}) (local.get $n)) (then {draw} {timer} (return (i32.const 0))))\n");
    }
    let wat = format!(
        r#"(module
          (import "figcraft" "reply_read" (func $reply_read (param i32 i32) (result i32)))
          {imports}
          (memory (export "memory") 4)
          {data}
          (func $has (param $p i32) (param $l i32) (param $n i32) (result i32) (local $i i32) (local $j i32)
            (block $no (loop $outer
              (br_if $no (i32.gt_s (i32.add (local.get $i) (local.get $l)) (local.get $n)))
              (local.set $j (i32.const 0))
              (block $miss (loop $inner
                (if (i32.ge_s (local.get $j) (local.get $l)) (then (return (i32.const 1))))
                (br_if $miss (i32.ne (i32.load8_u (i32.add (i32.const 100000) (i32.add (local.get $i) (local.get $j))))
                                     (i32.load8_u (i32.add (local.get $p) (local.get $j)))))
                (local.set $j (i32.add (local.get $j) (i32.const 1)))
                (br $inner)))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $outer)))
            (i32.const 0))
          (func (export "figcraft_run") (param $n i32) (result i32)
            (drop (call $reply_read (i32.const 100000) (local.get $n)))
            {call}
            {body}
            (i32.const 0)))"#
    );
    wat::parse_str(&wat).unwrap_or_else(|e| panic!("{e}\n{wat}"))
}

/// The note widget in state `n`: a title to edit, a part with a hover style and a tooltip, and a
/// property menu.
fn look(n: i64) -> Value {
    json!({"state": {"n": n}, "menu": [
        {"type": "action", "name": "reset", "label": "Start over"},
        {"type": "toggle", "name": "pinned", "label": "Pinned note"},
        {"type": "dropdown", "name": "size", "label": "Note size", "options": ["S", "M"], "value": "M"},
        {"type": "color", "name": "tint", "label": "Note tint", "options": ["#FF0000", "#00FF00"], "value": "#FF0000"},
        {"type": "separator"},
        {"type": "link", "name": "docs", "label": "Note docs", "url": "https://example.org/docs"}
    ], "root": {"type": "frame", "name": "Note", "layout": "vertical", "gap": 8, "padding": 12, "fill": "#FFFFFF", "align": "min", "children": [
        {"type": "text", "name": "Title", "text": format!("Title {n}"), "input": "title", "maxLength": 12, "width": 160, "size": 14},
        {"type": "rectangle", "name": "Go", "target": "go", "width": 80, "height": 30, "fill": "#0D99FF", "hover": {"fill": "#00FF00"}, "tooltip": "Go on"},
        {"type": "rectangle", "name": "Soon", "target": "soon", "width": 80, "height": 30, "fill": "#F24822"}
    ]}})
}

/// A session with the note widget's plugin: inserted it is in state 1; `go` draws 2 and asks for
/// a timer in a minute, `soon` draws 5 and asks for one in 100 ms, a timer draws 3, text typed
/// into the title draws 4, the menu's `reset` draws 0 and its other items draw nothing.
pub(crate) fn session() -> Session {
    let imports = r#"(import "figcraft" "widget_render" (func $draw (param i32 i32) (result i32)))
          (import "figcraft" "widget_timer" (func $timer (param i32) (result i32)))"#;
    let cases = [
        (r#""timer":true"#, look(3), -1),
        (r#""target":"go""#, look(2), 60_000),
        (r#""target":"soon""#, look(5), 100),
        (r#""input":"title""#, look(4), -1),
        (r#""menu":"reset""#, look(0), -1),
        (r#""menu":""#, Value::Null, -1),
        ("", look(1), -1),
    ];
    let manifest =
        json!({"id": NOTE, "name": "Note", "version": "1.0.0", "main": "w.wasm", "api": 4, "widget": true, "permissions": {"access": "read"}});
    let mut s = Session::new();
    let wasm = base64::engine::general_purpose::STANDARD.encode(module(imports, &cases, ""));
    s.execute("plugin.import", &json!({"manifest": manifest, "wasm": wasm})).unwrap();
    s
}

fn insert(s: &mut Session) -> NodeId {
    NodeId(s.execute("widget.insert", &json!({"plugin": NOTE, "x": 0, "y": 0})).unwrap()["id"].as_u64().unwrap())
}

pub(crate) fn state(doc: &Document, w: NodeId) -> String {
    doc.node(w).and_then(|n| n.plugin_data.get("figcraft.widget.state").cloned()).unwrap_or_default()
}

/// The layer called `name` inside widget `w`.
pub(crate) fn part(doc: &Document, w: NodeId, name: &str) -> NodeId {
    doc.subtree(w).into_iter().find(|i| doc.node(*i).is_some_and(|n| n.name == name)).unwrap_or_else(|| panic!("no {name}"))
}

/// Where the layer called `name` of widget `w` shows on the canvas.
fn on_canvas(app: &crate::FigApp, w: NodeId, name: &str) -> Rect {
    let doc = app.session.doc().unwrap();
    let b = doc.world_bounds(if name.is_empty() { w } else { part(doc, w, name) }).unwrap();
    Rect::from_two_pos(app.to_screen(b.x0, b.y0), app.to_screen(b.x1, b.y1))
}

pub(crate) fn fill(doc: &Document, id: NodeId) -> String {
    doc.node(id).unwrap().fills[0].solid_color().unwrap().to_css_hex()
}

pub(crate) fn key(k: Key) -> Vec<Event> {
    [true, false].map(|pressed| Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }).to_vec()
}

fn click(u: &mut Ui, at: Pos2) {
    u.button(at, true);
    u.button(at, false);
}

/// The AccessKit tree a screen reader would get now.
fn tree(u: &mut Ui) -> Vec<Node> {
    u.ctx.enable_accesskit();
    u.frame(vec![]);
    u.frame_output(vec![]).platform_output.accesskit_update.expect("an AccessKit update").nodes.into_iter().map(|(_, n)| n).collect()
}

fn bounds(n: &Node) -> Rect {
    let b = n.bounds().unwrap();
    Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32))
}

#[test]
fn a_widgets_text_is_typed_in_place_on_the_canvas_and_committed_to_its_plugin() {
    let mut s = session();
    let w = insert(&mut s);
    s.execute("select.set", &json!({"ids": []})).unwrap();
    let mut u = Ui::new(s);
    let undo = u.app.session.ds().unwrap().history.undo.len();
    let title = part(u.app.session.doc().unwrap(), w, "Title");
    let at = on_canvas(&u.app, w, "Title").center();
    click(&mut u, at);
    assert_eq!(u.app.session.ds().unwrap().text_edit.as_ref().map(|t| t.id), Some(title), "a click starts editing the text in place");
    // What is typed replaces the selected text and shows at once, up to the text's maximum.
    u.frame(vec![Event::Text("Hello, wide world".into())]);
    let shown = |u: &Ui| u.app.session.doc().unwrap().node(title).and_then(|n| n.kind.text()).map(|t| t.characters.clone());
    assert_eq!(shown(&u).as_deref(), Some("Hello, wide "));
    assert_eq!(state(u.app.session.doc().unwrap(), w), r#"{"n":1}"#, "the plugin hears nothing while it's typed");
    // Enter commits: the plugin is called back and redraws, as one undo step.
    u.frame(key(Key::Enter));
    u.frame(vec![]);
    let ds = u.app.session.ds().unwrap();
    assert!(ds.text_edit.is_none());
    assert_eq!((state(&ds.doc, w), ds.history.undo.len(), ds.selection.clone()), (r#"{"n":4}"#.to_string(), undo + 1, vec![w]));
}

#[test]
fn the_property_menu_sits_by_the_selected_widget_and_in_the_design_panel() {
    let mut s = session();
    let w = insert(&mut s);
    let mut u = Ui::new(s);
    let nodes = tree(&mut u);
    // The panel keeps its place beside the canvas (a row that overflows would push it out).
    assert!((u.app.canvas.rect.max.x - 1200.0).abs() < 1.0, "{:?}", u.app.canvas.rect);
    let (canvas, widget) = (u.app.canvas.rect, on_canvas(&u.app, w, ""));
    // Every item is there twice, named: in the bar over the canvas and in the Design panel.
    let named = |role: Role, label: &str| nodes.iter().filter(|n| n.role() == role && n.label() == Some(label)).map(bounds).collect::<Vec<_>>();
    // The bar sits just above the widget, or just under it when the canvas has no room above.
    let above = widget.min.y - 68.0 >= canvas.min.y;
    for (role, label) in [
        (Role::Button, "Start over"),
        (Role::CheckBox, "Pinned note"),
        (Role::ComboBox, "Note size"),
        (Role::RadioButton, "Note tint: #FF0000"),
        (Role::Link, "Note docs"),
    ] {
        let at = named(role, label);
        let (bar, panel): (Vec<&Rect>, Vec<&Rect>) = at.iter().partition(|r| canvas.contains_rect(**r));
        assert_eq!((bar.len(), panel.len()), (1, 1), "{label}: one over the canvas, one in the panel: {at:?} (canvas {canvas:?})");
        let gap = if above { widget.min.y - bar[0].max.y } else { bar[0].min.y - widget.max.y };
        assert!((0.0..60.0).contains(&gap), "{label} at {:?}, the widget at {widget:?} (above: {above})", bar[0]);
        assert!(panel[0].min.x >= canvas.max.x, "{label} in the Design panel: {:?}", panel[0]);
    }
    assert!(nodes.iter().any(|n| n.role() == Role::Toolbar && n.label() == Some("Widget")));
    let bar_reset = named(Role::Button, "Start over").into_iter().find(|r| canvas.contains_rect(*r)).unwrap();
    assert!((bar_reset.center().x - widget.center().x).abs() < 300.0, "over the widget's middle (or as near as the canvas allows)");
    // Using an item calls the plugin back; a toggle shows its new state.
    click(&mut u, bar_reset.center());
    assert_eq!(state(u.app.session.doc().unwrap(), w), r#"{"n":0}"#);
    let pinned = nodes.iter().filter(|n| n.label() == Some("Pinned note")).map(bounds).find(|r| !canvas.contains_rect(*r)).unwrap();
    click(&mut u, pinned.center());
    let menu = u.app.session.execute("widget.menu", &json!({"id": w})).unwrap();
    assert_eq!(menu["items"][1]["value"], json!(true), "the Design panel's toggle");
    // The Design panel's items are reached and used with the keyboard: F6 to the panel, then Tab,
    // and Enter on the action.
    u.app.session.execute("widget.click", &json!({"id": w, "target": "go"})).unwrap();
    for _ in 0..4 {
        if u.app.ui.nav.region == Region::Properties {
            break;
        }
        u.frame(key(Key::F6));
    }
    assert_eq!(u.app.ui.nav.region, Region::Properties);
    let mut seen = Vec::new();
    for _ in 0..60 {
        seen.push(crate::a11y::focused_name(&u.ctx));
        if seen.last().is_some_and(|n| n == "Start over") {
            u.frame(key(Key::Enter));
            u.frame(vec![]);
            assert_eq!(state(u.app.session.doc().unwrap(), w), r#"{"n":0}"#, "used from the keyboard");
        }
        u.frame(key(Key::Tab));
    }
    for want in ["Start over", "Pinned note", "Note size", "Note docs"] {
        assert!(seen.iter().any(|n| n == want), "{want} not reached by Tab: {seen:?}");
    }
    // Deselected, there is no menu anywhere.
    u.app.session.execute("select.set", &json!({"ids": []})).unwrap();
    assert!(!tree(&mut u).iter().any(|n| n.label() == Some("Start over")));
}

#[test]
fn a_parts_hover_style_and_tooltip_show_without_a_callback_or_more_frames() {
    let mut s = session();
    let w = insert(&mut s);
    s.execute("select.set", &json!({"ids": []})).unwrap();
    let mut h = Harness::new(s);
    h.settle();
    let go = part(h.app.session.doc().unwrap(), w, "Go");
    let at = on_canvas(&h.app, w, "Go").center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![]);
    let ds = h.app.session.ds().unwrap();
    assert_eq!((fill(&ds.doc, go), fill(ds.document_itself(), go)), ("#00FF00".to_string(), "#0D99FF".to_string()), "shown, not in the document");
    assert_eq!((h.app.session.tool.overlay.widget_tip.as_deref(), ds.history.undo.len()), (Some("Go on"), 1));
    assert_eq!(state(&ds.doc, w), r#"{"n":1}"#, "its plugin wasn't called");
    // Resting there draws nothing more.
    h.assert_idle("a hovered widget part");
    assert_eq!(fill(&h.app.session.ds().unwrap().doc, go), "#00FF00");
    // Moving off puts it back, on the canvas and when the pointer leaves it for a panel.
    h.frame(vec![Event::PointerMoved(at + egui::vec2(0.0, 300.0))]);
    let ds = h.app.session.ds().unwrap();
    assert!(fill(&ds.doc, go) == "#0D99FF" && ds.text_preview.is_none() && h.app.session.tool.overlay.widget_tip.is_none());
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![]);
    assert_eq!(fill(&h.app.session.ds().unwrap().doc, go), "#00FF00");
    h.frame(vec![Event::PointerMoved(egui::pos2(1300.0, 500.0))]);
    h.frame(vec![]);
    let ds = h.app.session.ds().unwrap();
    assert!(fill(&ds.doc, go) == "#0D99FF" && ds.text_preview.is_none() && h.app.session.tool.overlay.widget_tip.is_none(), "over the Design panel");
}

#[test]
fn a_pending_timer_wakes_the_window_when_it_is_due_and_at_no_other_time() {
    let mut s = session();
    let w = insert(&mut s);
    let mut h = Harness::new(s);
    // No timer: the next frame is the autosave's, minutes away.
    assert!(h.settle() > Duration::from_secs(60));
    // A timer a minute off: the window sleeps until then, and draws nothing meanwhile.
    h.run("widget.click", json!({"id": w, "target": "go"}));
    let wake = h.settle();
    assert!(wake > Duration::from_secs(30) && wake <= Duration::from_secs(60), "{wake:?}");
    h.assert_idle("a widget with a timer pending");
    assert_eq!(state(h.app.session.doc().unwrap(), w), r#"{"n":2}"#);
    // One due in 100 ms fires in the frame after that: its plugin is called back.
    h.run("widget.click", json!({"id": w, "target": "soon"}));
    assert!(h.frame(vec![]) <= Duration::from_millis(100));
    assert_eq!(state(h.app.session.doc().unwrap(), w), r#"{"n":5}"#, "not before it is due");
    std::thread::sleep(Duration::from_millis(130));
    h.frame(vec![]);
    assert_eq!(state(h.app.session.doc().unwrap(), w), r#"{"n":3}"#);
    // It asked for no other: the window is idle again, with nothing to wake for.
    assert!(h.settle() > Duration::from_secs(60));
    // While a prototype is presented the file's timers wait (the prototype plays its own copy).
    let a = h.run("frame.create", json!({"x": 400, "y": 0, "width": 300, "height": 300}))["id"].clone();
    h.run("widget.click", json!({"id": w, "target": "soon"}));
    h.run("proto.present", json!({"start": a}));
    std::thread::sleep(Duration::from_millis(130));
    h.frame(vec![]);
    assert_eq!(state(h.app.session.doc().unwrap(), w), r#"{"n":5}"#);
    h.app.ui.present = None;
    h.frame(vec![]);
    assert_eq!(state(h.app.session.doc().unwrap(), w), r#"{"n":3}"#, "it fires once the presentation ends");
}

#[test]
fn a_panels_slider_tells_its_plugin_now_and_then_while_it_is_dragged() {
    use crate::plugins_ui::slider_due;
    assert!(slider_due(false, 5.0, None) && slider_due(true, 5.0, Some(4.99)), "the first move and the last always tell");
    assert!(!slider_due(false, 5.05, Some(5.0)) && slider_due(false, 5.2, Some(5.0)));
    // A panel with a slider, a picture, a progress bar and a section; each event makes a rectangle.
    let spec = json!({"title": "Dots", "controls": [
        {"type": "slider", "name": "amount", "label": "Amount", "min": 0, "max": 100, "step": 1, "value": 0},
        {"type": "image", "label": "Preview", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "height": 40},
        {"type": "progress", "label": "Half done", "value": 0.5},
        {"type": "divider"},
        {"type": "section", "label": "Fine tuning", "value": false},
        {"type": "text", "name": "hidden", "label": "Hidden", "error": "Needs a name"},
        {"type": "button", "name": "go", "label": "Go", "disabled": true},
        {"type": "divider"},
        {"type": "note", "label": "Always shown"}
    ]});
    // Its strings: the command, its parameters, the needle that tells an event from the first run,
    // and the panel.
    let hex = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    let (cmd, rect, needle, text) = ("shape.rectangle", r#"{"x":0,"y":0,"width":5,"height":5}"#, r#""event""#, spec.to_string());
    let imports = format!(
        r#"(import "figcraft" "ui_show" (func $show (param i32 i32) (result i32)))
          (import "figcraft" "call" (func $call (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 200) "{}") (data (i32.const 300) "{}") (data (i32.const 400) "{}") (data (i32.const 50000) "{}")"#,
        hex(cmd),
        hex(rect),
        hex(needle),
        hex(&text)
    );
    let call = format!(
        "(if (i32.eqz (call $has (i32.const 400) (i32.const {}) (local.get $n))) (then (return (call $show (i32.const 50000) (i32.const {})))))
         (drop (call $call (i32.const 200) (i32.const {}) (i32.const 300) (i32.const {})))",
        needle.len(),
        text.len(),
        cmd.len(),
        rect.len()
    );
    let manifest = json!({"id": "org.test.dots", "name": "Dots", "main": "p.wasm", "api": 4, "permissions": {"access": "write"}});
    let mut s = Session::new();
    let wasm = base64::engine::general_purpose::STANDARD.encode(module(&imports, &[], &call));
    s.execute("plugin.import", &json!({"manifest": manifest, "wasm": wasm})).unwrap();
    s.execute("plugin.run", &json!({"id": "org.test.dots"})).unwrap();
    let mut u = Ui::new(s);
    let nodes = tree(&mut u);
    let find = |role: Role, label: &str| nodes.iter().find(|n| n.role() == role && n.label() == Some(label)).map(bounds);
    let track = find(Role::Slider, "Amount").expect("the slider is named");
    assert!(
        find(Role::Image, "Preview").is_some() && find(Role::ProgressIndicator, "Half done").is_some(),
        "the picture and the progress bar are named"
    );
    assert!(nodes.iter().all(|n| n.label() != Some("Hidden") && n.label() != Some("Go")), "a closed section hides its controls");
    assert!(nodes.iter().any(|n| n.label().or(n.value()) == Some("Always shown")), "a divider ends the section");
    let layers = |u: &Ui| {
        let ds = u.app.session.ds().unwrap();
        ds.doc.node(ds.page).unwrap().children.len()
    };
    // Dragged across in 40 frames (two thirds of a second): the plugin hears a handful of times,
    // not forty, the slider follows the pointer all the way, and the last value is told.
    let (from, to) = (Pos2::new(track.min.x + 8.0, track.center().y), Pos2::new(track.max.x - 8.0, track.center().y));
    u.button(from, true);
    for k in 1..=40 {
        u.frame(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 40.0))]);
    }
    let during = layers(&u);
    assert!((3..=9).contains(&during), "told {during} times while dragging");
    u.button(to, false);
    u.frame(vec![]);
    let value = u.app.session.plugins.panel("org.test.dots").unwrap().values["amount"].as_f64().unwrap();
    assert!(value >= 95.0 && layers(&u) <= during + 1, "the plugin has the value the drag ended on: {value}");
    // Opening the section shows its controls: the validation message, and a button that can't be used.
    let more = nodes.iter().find(|n| n.label() == Some("Fine tuning")).map(bounds).expect("the section's heading");
    click(&mut u, more.center());
    let nodes = tree(&mut u);
    let labels: Vec<&str> = nodes.iter().filter_map(|n| n.label().or(n.value())).collect();
    assert!(labels.contains(&"Hidden") && labels.contains(&"Needs a name"), "the field and its message: {labels:?}");
    let go = nodes.iter().find(|n| n.label() == Some("Go")).map(bounds).unwrap();
    let before = layers(&u);
    click(&mut u, go.center());
    assert_eq!(layers(&u), before, "a disabled button calls nobody");
}
