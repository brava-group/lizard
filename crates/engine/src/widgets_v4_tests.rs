//! Plugin API 4: widgets take text typed in place, items of their property menu and timers, each
//! one callback and one undo step (a run of timer ticks is one); hover styles and tooltips show
//! without the plugin or the undo history; lines, vectors, gradients, shadows, text styles and
//! fit modes are built; panels get sliders, images, progress bars, dividers, sections, disabled
//! controls and validation messages. Everything is kept through save, reload, duplicating and
//! export, syncs in a live session (where a timer runs for whoever started it last), works on a
//! prototype's copy, is refused for plugins written for API 1–3, and junk never panics.
//! Test plugins are WAT (see `widgets_tests`): they output their input, then draw what it asks.

use std::sync::{Arc, Mutex};

use figcraft_collab::message::{ClientMsg, ServerMsg};
use figcraft_collab::{Client, Outgoing, Server};
use figcraft_color::{ImageScaleMode, PaintKind};
use figcraft_doc::{EffectKind, NodeId, NodeKind};
use serde_json::{Value, json};

use crate::components::tests::{ex, idv, node};
use crate::live::{Access, Event, Role};
use crate::live_tests::{Script, info, push};
use crate::plugins_tests::{install, undo_len};
use crate::tools::{Cursor, PointerEvent, PointerKind};
use crate::widgets::{next_timer, run_on, run_timers};
use crate::widgets_tests::{W, err, state, widget_manifest};
use crate::{EngineError, Session};

const CARD: &str = "org.test.card";
const TIMER: &str = r#"(import "figcraft" "widget_timer" (func $timer (param i32) (result i32)))"#;
/// A 1×1 PNG.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

fn menu() -> Value {
    json!([
        {"type": "action", "name": "reset", "label": "Reset"},
        {"type": "toggle", "name": "pinned", "label": "Pinned"},
        {"type": "dropdown", "name": "size", "label": "Size", "options": ["S", "M", "L"], "value": "M"},
        {"type": "color", "name": "tint", "label": "Tint", "options": ["#ff0000", "#00FF00"], "value": "#00ff00"},
        {"type": "separator"},
        {"type": "link", "name": "docs", "label": "Docs", "url": "https://example.org/docs"}
    ])
}

/// A widget using everything API 4 added, in state `n`, labelled `label`.
fn look(n: i64, label: &str) -> Value {
    json!({"state": {"n": n}, "menu": menu(), "root": {"type": "frame", "name": "Card", "layout": "vertical", "gap": 6, "padding": [4, 8, 12, 16],
        "radius": [1, 2, 3, 4], "width": 200, "align": "min",
        "gradient": {"type": "linear", "angle": 0, "stops": [{"at": 0, "color": "#ffffff"}, {"at": 1, "color": "#dddddd"}]},
        "shadow": {"x": 1, "y": 2, "blur": 6, "color": "#00000040"}, "children": [
        {"type": "text", "name": "Title", "text": label, "input": "title", "maxLength": 10, "width": 120, "weight": 700, "textAlign": "center",
         "maxLines": 2, "size": 12, "tooltip": "Rename", "hover": {"fill": "#FF0000"}},
        {"type": "text", "name": "Notes", "text": "notes", "input": "notes", "maxLength": 40, "multiline": true, "width": 120, "size": 12},
        {"type": "rectangle", "name": "Start", "target": "start", "height": 20, "fill": "#0D99FF", "fillWidth": true, "tooltip": "Start",
         "hover": {"fill": "#00FF00", "stroke": "#000000", "opacity": 0.5}},
        {"type": "ellipse", "name": "Stop", "target": "stop", "width": 20, "height": 20, "fill": "#F24822"},
        {"type": "line", "width": 50},
        {"type": "vector", "path": "M0 0 L10 0 L5 8 Z", "fill": "#111111", "width": 20},
        {"type": "image", "data": PIXEL, "width": 16, "height": 16, "fit": "tile"}
    ]}})
}

/// The test widget: it draws "new" (state 1) when inserted; `start` asks for a 100 ms timer and
/// draws 2, `stop` cancels it without drawing, a timer draws 3 and asks again, text typed into
/// `title` draws 4 (44 when it is "Hello ther") and into `notes` nothing, the menu's `reset`
/// draws 0 and its other items draw nothing. `extra` runs first on every call.
fn card(extra: impl FnOnce(&mut W) -> String) -> Vec<u8> {
    let mut w = W::default();
    w.imports = TIMER.into();
    let pre = extra(&mut w);
    let on = |w: &mut W, needle: &str, then: String| format!("(if {} (then {then} (return (i32.const 0))))", w.has(needle));
    let again = "(drop (call $timer (i32.const 100)))";
    let (tick, started, exact, typed, reset, new) = (
        w.render(&look(3, "tick")),
        w.render(&look(2, "started")),
        w.render(&look(44, "exact")),
        w.render(&look(4, "typed")),
        w.render(&look(0, "reset")),
        w.render(&look(1, "new")),
    );
    let body = [
        on(&mut w, r#""timer":true"#, format!("{tick} {again}")),
        on(&mut w, r#""target":"start""#, format!("{started} {again}")),
        on(&mut w, r#""target":"stop""#, "(drop (call $timer (i32.const 0)))".into()),
        on(&mut w, r#""text":"Hello ther""#, exact),
        on(&mut w, r#""input":"title""#, typed),
        on(&mut w, r#""input":"notes""#, String::new()),
        on(&mut w, r#""menu":"reset""#, reset),
        on(&mut w, r#""menu":""#, String::new()),
    ]
    .join("\n");
    w.build(&format!("{pre} {body} {new}"))
}

fn manifest(api: u32) -> Value {
    let mut m = widget_manifest(CARD, "read");
    m["api"] = json!(api);
    m
}

fn session() -> Session {
    let mut s = Session::new();
    install(&mut s, manifest(4), &card(|_| String::new()));
    s
}

fn insert(s: &mut Session) -> (NodeId, Value) {
    let r = ex(s, "widget.insert", json!({"plugin": CARD, "x": 40, "y": 40}));
    (idv(&r), r)
}

/// The layer called `name` inside widget `w`.
fn part(s: &Session, w: NodeId, name: &str) -> NodeId {
    node(s, w).children.iter().copied().find(|c| node(s, *c).name == name).unwrap_or_else(|| panic!("no {name}"))
}

fn text_of(s: &Session, id: NodeId) -> String {
    node(s, id).kind.text().unwrap().characters.clone()
}

fn pointer(s: &mut Session, kind: PointerKind, id: NodeId) {
    let c = s.doc().unwrap().world_bounds(id).unwrap().center();
    s.pointer(&PointerEvent::at(kind, c.x, c.y, 1.0)).unwrap();
}

/// A pointer event over the layer called `name` of widget `w`.
fn point(s: &mut Session, kind: PointerKind, w: NodeId, name: &str) {
    let id = part(s, w, name);
    pointer(s, kind, id);
}

fn listed(s: &mut Session, w: NodeId) -> Value {
    let list = ex(s, "widget.list", json!({}));
    list["widgets"].as_array().unwrap().iter().find(|x| x["id"] == json!(w)).cloned().unwrap()
}

#[test]
fn text_typed_in_place_or_set_by_command_calls_the_plugin_back_as_one_undo_step() {
    let mut s = session();
    let (w, r) = insert(&mut s);
    assert_eq!(r["output"]["api"], json!(4));
    assert_eq!(
        r["inputs"],
        json!([{"name": "title", "text": "new", "maxLength": 10, "multiline": false}, {"name": "notes", "text": "notes", "maxLength": 40, "multiline": true}])
    );
    // By command: cut to the maximum, one line; the plugin gets the name and the text and redraws.
    let undo = undo_len(&s);
    let r = ex(&mut s, "widget.setText", json!({"id": w, "input": "title", "text": "A\tlong\nline of text"}));
    assert_eq!(r["output"]["widget"]["event"], json!({"input": "title", "text": "A long lin"}));
    assert_eq!((r["state"].clone(), undo_len(&s), text_of(&s, part(&s, w, "Title"))), (json!({"n": 4}), undo + 1, "typed".into()));
    let inside = part(&s, w, "Start");
    let r = ex(&mut s, "widget.setText", json!({"id": inside, "input": "notes", "text": "two\r\nlines\u{7}"}));
    assert_eq!(r["output"]["widget"]["event"], json!({"input": "notes", "text": "two\nlines"}), "a multiline text keeps its line breaks");
    assert_eq!(undo_len(&s), undo + 1, "a callback that changes nothing is no undo step");
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!((state(&mut s, w), text_of(&s, part(&s, w, "Title"))), (json!({"n": 1}), "new".into()));
    ex(&mut s, "edit.redo", json!({}));
    assert_eq!(state(&mut s, w), json!({"n": 4}));
    ex(&mut s, "edit.undo", json!({}));
    assert!(err(&mut s, "widget.setText", json!({"id": w, "input": "nope", "text": "x"})).contains("it has: title, notes"));
    assert!(err(&mut s, "widget.setText", json!({"id": w, "input": "title"})).contains("`text` required"));
    assert!(err(&mut s, "widget.setText", json!({"id": w, "input": "title", "text": 5})).contains("`text` required"));

    // On the canvas: an I-beam over the text, a click edits it in place with everything selected.
    s.execute("select.set", &json!({"ids": []})).unwrap();
    let title = part(&s, w, "Title");
    pointer(&mut s, PointerKind::Move, title);
    assert_eq!(s.tool.overlay.cursor, Cursor::Text);
    let undo = undo_len(&s);
    pointer(&mut s, PointerKind::Down, title);
    pointer(&mut s, PointerKind::Up, title);
    let te = s.ds().unwrap().text_edit.clone().unwrap();
    assert_eq!((te.id, te.anchor, te.caret), (title, 0, 3));
    // Typing replaces it, shows at once, and stops at the maximum; Enter is no line break.
    crate::textedit::insert(&mut s, "Hello\nthere, world").unwrap();
    assert_eq!(text_of(&s, title), "Hello ther");
    crate::textedit::insert(&mut s, "!").unwrap();
    assert_eq!((text_of(&s, title), undo_len(&s)), ("Hello ther".into(), undo));
    crate::textedit::key(&mut s, "Enter", false, false, false).unwrap();
    // Committed: the plugin saw exactly that text and redrew; one undo step; the widget is selected.
    assert!(s.ds().unwrap().text_edit.is_none() && s.ds().unwrap().interaction.is_none());
    assert_eq!((state(&mut s, w), undo_len(&s), s.selection()), (json!({"n": 44}), undo + 1, vec![w]));
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!((state(&mut s, w), text_of(&s, part(&s, w, "Title"))), (json!({"n": 1}), "new".into()));

    // Leaving it as it was calls nobody; a multiline text takes Enter and commits on Escape.
    let title = part(&s, w, "Title");
    pointer(&mut s, PointerKind::Down, title);
    pointer(&mut s, PointerKind::Up, title);
    crate::textedit::key(&mut s, "Escape", false, false, false).unwrap();
    assert_eq!((state(&mut s, w), undo_len(&s)), (json!({"n": 1}), undo));
    let notes = part(&s, w, "Notes");
    crate::textedit::begin(&mut s, notes, None, false).unwrap();
    crate::textedit::insert(&mut s, "a").unwrap();
    crate::textedit::key(&mut s, "Enter", false, false, false).unwrap();
    crate::textedit::insert(&mut s, "- b").unwrap();
    assert_eq!(text_of(&s, notes), "a\n- b", "no lists in a widget's text");
    crate::textedit::key(&mut s, "Escape", false, false, false).unwrap();
    assert_eq!((text_of(&s, part(&s, w, "Notes")), undo_len(&s)), ("notes".into(), undo), "the plugin drew nothing: the layer is as it was");
}

#[test]
fn the_property_menu_is_listed_and_its_items_call_the_plugin_back() {
    let mut s = session();
    let (w, r) = insert(&mut s);
    let inside = part(&s, w, "Start");
    let m = ex(&mut s, "widget.menu", json!({"id": inside}));
    assert_eq!((m["id"].clone(), m["plugin"].clone(), m["items"].clone()), (json!(w), json!(CARD), r["menu"].clone()));
    let items = m["items"].as_array().unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(items[1], json!({"type": "toggle", "name": "pinned", "label": "Pinned", "value": false}));
    assert_eq!(items[3], json!({"type": "color", "name": "tint", "label": "Tint", "value": "#00FF00", "options": ["#FF0000", "#00FF00"]}));
    // An action: no value; the plugin redraws; one undo step.
    ex(&mut s, "widget.click", json!({"id": w, "target": "stop"}));
    let undo = undo_len(&s);
    let r = ex(&mut s, "widget.menuAction", json!({"id": w, "item": "reset", "value": "ignored"}));
    assert_eq!(r["output"]["widget"]["event"], json!({"menu": "reset", "value": null}));
    assert_eq!((r["state"].clone(), undo_len(&s)), (json!({"n": 0}), undo + 1));
    // A toggle flips unless told; a dropdown and a colour take one of their options. The menu
    // shows the choice though this plugin draws nothing for them.
    let value = |s: &mut Session, item: &str, v: Value| {
        let mut p = json!({"id": w, "item": item});
        if !v.is_null() {
            p["value"] = v;
        }
        ex(s, "widget.menuAction", p)["output"]["widget"]["event"]["value"].clone()
    };
    assert_eq!(value(&mut s, "pinned", Value::Null), json!(true));
    assert_eq!(value(&mut s, "pinned", Value::Null), json!(false));
    assert_eq!(value(&mut s, "pinned", json!(true)), json!(true));
    assert_eq!(value(&mut s, "size", json!("L")), json!("L"));
    assert_eq!(value(&mut s, "tint", json!("#ff0000")), json!("#FF0000"));
    let now = ex(&mut s, "widget.menu", json!({"id": w}))["items"].clone();
    assert_eq!((now[1]["value"].clone(), now[2]["value"].clone(), now[3]["value"].clone()), (json!(true), json!("L"), json!("#FF0000")));
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!(ex(&mut s, "widget.menu", json!({"id": w}))["items"][3]["value"], json!("#00FF00"), "choosing is an undo step");
    // A link is the app's to open: the plugin isn't called and nothing changes.
    let undo = undo_len(&s);
    assert_eq!(
        ex(&mut s, "widget.menuAction", json!({"id": w, "item": "docs"})),
        json!({"id": w, "item": "docs", "url": "https://example.org/docs"})
    );
    assert_eq!(undo_len(&s), undo);
    for (p, why) in [
        (json!({"id": w, "item": "nope"}), "it has: reset, pinned, size, tint, docs"),
        (json!({"id": w}), "`item` required"),
        (json!({"id": w, "item": "size", "value": "XL"}), "must be one of: S, M, L"),
        (json!({"id": w, "item": "size"}), "must be one of"),
        (json!({"id": w, "item": "tint", "value": "#123456"}), "must be one of"),
        (json!({"id": w, "item": "pinned", "value": "yes"}), "true or false"),
    ] {
        assert!(err(&mut s, "widget.menuAction", p.clone()).contains(why), "{p}");
    }
    let r = crate::plugins_tests::rect(&mut s, 900.0);
    assert!(err(&mut s, "widget.menu", json!({"id": r})).contains("isn't a widget"));
}

#[test]
fn hover_styles_and_tooltips_show_without_the_plugin_or_an_undo_step() {
    let mut s = session();
    let (w, _) = insert(&mut s);
    s.execute("select.set", &json!({"ids": []})).unwrap();
    let (start, stop) = (part(&s, w, "Start"), part(&s, w, "Stop"));
    let itself = s.ds().unwrap().doc.clone();
    let undo = undo_len(&s);
    let shown = |s: &Session| {
        let n = node(s, start);
        (n.fills[0].solid_color().unwrap().to_css_hex(), n.strokes.len(), n.opacity)
    };
    assert_eq!(shown(&s), ("#0D99FF".into(), 0, 1.0));
    pointer(&mut s, PointerKind::Move, start);
    assert_eq!(shown(&s), ("#00FF00".into(), 1, 0.5), "the hover style shows");
    assert_eq!((s.tool.overlay.cursor, s.tool.overlay.widget_tip.as_deref()), (Cursor::Pointer, Some("Start")));
    let ds = s.ds().unwrap();
    assert!(Arc::ptr_eq(ds.document_itself(), &itself) && undo_len(&s) == undo, "the document and its history are as they were");
    // The same part again changes nothing; another part, or none, puts it back.
    let revision = ds.revision;
    pointer(&mut s, PointerKind::Move, start);
    assert_eq!(s.ds().unwrap().revision, revision);
    pointer(&mut s, PointerKind::Move, stop);
    assert!(Arc::ptr_eq(&s.ds().unwrap().doc, &itself) && s.tool.overlay.widget_tip.is_none(), "a part without a hover style or a tooltip");
    assert_eq!(s.tool.overlay.cursor, Cursor::Pointer);
    // A text's hover colours it; any command ends the look, and a click still works.
    point(&mut s, PointerKind::Move, w, "Title");
    assert_eq!(node(&s, part(&s, w, "Title")).fills[0].solid_color().unwrap().to_css_hex(), "#FF0000");
    assert_eq!(s.tool.overlay.widget_tip.as_deref(), Some("Rename"));
    ex(&mut s, "widget.list", json!({}));
    assert!(Arc::ptr_eq(&s.ds().unwrap().doc, &itself));
    pointer(&mut s, PointerKind::Move, start);
    pointer(&mut s, PointerKind::Down, start);
    pointer(&mut s, PointerKind::Up, start);
    assert_eq!((state(&mut s, w), undo_len(&s)), (json!({"n": 2}), undo + 1));
    assert!(s.ds().unwrap().text_preview.is_none());
    // Saving while a part is hovered saves the document itself.
    point(&mut s, PointerKind::Move, w, "Start");
    assert!(s.ds().unwrap().text_preview.is_some());
    let fill = |d: &figcraft_doc::Document| d.node(part(&s, w, "Start")).unwrap().fills[0].solid_color().unwrap().to_css_hex();
    assert_eq!(fill(s.ds().unwrap().document_itself()), "#0D99FF");
}

#[test]
fn a_timer_calls_back_once_due_and_a_run_of_ticks_is_one_undo_step() {
    let mut s = session();
    let (w, r) = insert(&mut s);
    assert_eq!((r["timer"].clone(), next_timer(&s)), (Value::Null, None));
    assert!(matches!(s.execute("widget.fireTimer", &json!({"id": w})), Err(EngineError::Disabled(..))), "no timer is pending");
    assert!(run_timers(&mut s).is_empty());
    // A callback asks for one: it is pending, and nothing happens before it is due.
    let r = ex(&mut s, "widget.click", json!({"id": w, "target": "start"}));
    let left = r["timer"].as_u64().unwrap();
    assert!(left <= 100 && next_timer(&s).is_some_and(|t| t <= 0.1), "{left}");
    assert_eq!(listed(&mut s, w)["timer"].as_u64().map(|t| t <= 100), Some(true));
    let undo = undo_len(&s);
    assert!(run_timers(&mut s).is_empty());
    assert_eq!((state(&mut s, w), undo_len(&s)), (json!({"n": 2}), undo));
    // Due: the plugin is called with the timer, redraws and asks again.
    std::thread::sleep(std::time::Duration::from_millis(130));
    assert!(run_timers(&mut s).is_empty());
    assert_eq!((state(&mut s, w), undo_len(&s)), (json!({"n": 3}), undo + 1));
    assert_eq!(s.undo_labels(1), vec!["Tally timer".to_string()]);
    assert!(next_timer(&s).is_some(), "it asked for the next one");
    // More ticks join that step (fired by command, as a script would).
    let r = ex(&mut s, "widget.fireTimer", json!({"id": w}));
    assert_eq!(r["output"]["widget"]["event"], json!({"timer": true}));
    ex(&mut s, "widget.fireTimer", json!({"id": w}));
    assert_eq!(undo_len(&s), undo + 1);
    // Undoing the ticks goes back to before the first; undoing the click that started the timer
    // stops it (the layer no longer carries this session's stamp).
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!(state(&mut s, w), json!({"n": 2}));
    ex(&mut s, "edit.undo", json!({}));
    assert_eq!((state(&mut s, w), listed(&mut s, w)["timer"].clone()), (json!({"n": 1}), Value::Null));
    std::thread::sleep(std::time::Duration::from_millis(130));
    assert!(run_timers(&mut s).is_empty());
    assert_eq!((state(&mut s, w), next_timer(&s)), (json!({"n": 1}), None), "the dead timer is dropped, not fired");
    // Cancelling, and a busy document: the timer waits for a drag or typing to end.
    ex(&mut s, "widget.click", json!({"id": w, "target": "start"}));
    ex(&mut s, "widget.click", json!({"id": w, "target": "stop"}));
    assert_eq!(next_timer(&s), None);
    ex(&mut s, "widget.click", json!({"id": w, "target": "start"}));
    let notes = part(&s, w, "Notes");
    crate::textedit::begin(&mut s, notes, None, false).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(130));
    assert!(run_timers(&mut s).is_empty());
    assert_eq!(state(&mut s, w), json!({"n": 2}));
    assert!(next_timer(&s).is_some_and(|t| t > 0.1), "it tries again a little later");
    crate::textedit::key(&mut s, "Escape", false, false, false).unwrap();
    assert_eq!(ex(&mut s, "widget.fireTimer", json!({"id": w}))["state"], json!({"n": 3}));
    // A copy has no timer of its own; a deleted widget's goes with it.
    s.execute("select.set", &json!({"ids": [w]})).unwrap();
    ex(&mut s, "edit.duplicate", json!({}));
    let copy = s.selection()[0];
    assert!(listed(&mut s, copy)["timer"].is_null() && listed(&mut s, w)["timer"].is_u64());
    assert!(matches!(s.execute("widget.fireTimer", &json!({"id": copy})), Err(EngineError::Disabled(..))));
    ex(&mut s, "edit.delete", json!({"ids": [w]}));
    std::thread::sleep(std::time::Duration::from_millis(130));
    assert!(run_timers(&mut s).is_empty());
    assert_eq!(next_timer(&s), None);
}

#[test]
fn timers_are_bounded_and_need_api_4() {
    let ask = |ms: i64| {
        move |w: &mut W| {
            let _ = w;
            format!(
                "(if (call $timer (i32.const {ms})) (then (call $set_output (i32.const 200000) (call $reply_read (i32.const 200000) (i32.const 5000))) (return (i32.const 1))))"
            )
        }
    };
    for (ms, api, why) in [
        (99, 4, "100 to 3600000"),
        (3_600_001, 4, "100 to 3600000"),
        (-1, 4, "100 to 3600000"),
        (i64::from(i32::MIN), 4, "100 to 3600000"),
        (1000, 3, "need plugin API 4"),
    ] {
        let mut s = Session::new();
        install(&mut s, manifest(api), &card(ask(ms)));
        let e = err(&mut s, "widget.insert", json!({"plugin": CARD}));
        assert!(e.contains(why), "{ms}: {e}");
        assert_eq!(crate::plugins_tests::layer_count(&s), 0);
    }
    for ms in [100, 3_600_000, 0] {
        let mut s = Session::new();
        install(&mut s, manifest(4), &card(ask(ms)));
        let r = ex(&mut s, "widget.insert", json!({"plugin": CARD}));
        assert_eq!(r["timer"].is_u64(), ms > 0, "{ms}");
    }
}

#[test]
fn richer_layers_are_built() {
    let mut s = session();
    let (w, r) = insert(&mut s);
    assert_eq!(r["targets"], json!(["start", "stop"]));
    let root = node(&s, w);
    let PaintKind::Gradient(g) = &root.fills[0].kind else { panic!("{:?}", root.fills) };
    assert_eq!((g.stops.len(), g.from, g.to), (2, (0.0, 0.5), (1.0, 0.5)), "0° runs left to right");
    let EffectKind::DropShadow(shadow) = &root.effects[0].kind else { panic!("{:?}", root.effects) };
    assert_eq!((shadow.offset, shadow.radius), ((1.0, 2.0), 6.0));
    assert_eq!(root.corners.radii, [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(root.layout.as_ref().unwrap().padding, [4.0, 8.0, 12.0, 16.0]);
    let title = node(&s, part(&s, w, "Title"));
    let t = title.kind.text().unwrap();
    assert_eq!(
        (t.max_lines, format!("{:?}", t.align_h).to_lowercase(), format!("{:?}", t.auto_resize).to_lowercase()),
        (2, "center".into(), "truncate".into())
    );
    assert!(serde_json::to_string(t).unwrap().contains("Bold"), "weight 700 is bold");
    let start = node(&s, part(&s, w, "Start"));
    assert!((start.width - (200.0 - 8.0 - 16.0)).abs() < 1e-6, "it fills the frame's width: {}", start.width);
    let line = node(&s, part(&s, w, "Line"));
    assert!(matches!(line.kind, NodeKind::Line) && line.width == 50.0 && line.strokes.len() == 1);
    let vector = node(&s, part(&s, w, "Vector"));
    let NodeKind::Vector { network } = &vector.kind else { panic!("{:?}", vector.kind) };
    assert!(
        network.vertices.len() >= 3 && (vector.width - 20.0).abs() < 1e-6 && (vector.height - 8.0).abs() < 1e-6,
        "{} × {}",
        vector.width,
        vector.height
    );
    let PaintKind::Image(image) = &node(&s, part(&s, w, "Image")).fills[0].kind else { panic!() };
    assert_eq!(image.scale_mode, ImageScaleMode::Tile);
    assert!(node(&s, w).children.iter().all(|c| node(&s, *c).locked));
    // Drawn again, nothing piles up.
    ex(&mut s, "widget.menuAction", json!({"id": w, "item": "reset"}));
    assert_eq!((node(&s, w).effects.len(), node(&s, w).fills.len(), node(&s, w).children.len()), (1, 1, 7));
    // A path that isn't one, or that leaves the limits, is refused and nothing is left behind.
    for path in ["M0 0 L1e9 0", "M 0 0 X 5"] {
        let mut b = W::default();
        let body = b.render(&json!({"root": {"type": "frame", "children": [{"type": "vector", "path": path}]}}));
        let mut s = Session::new();
        install(&mut s, manifest(4), &b.build(&body));
        let e = err(&mut s, "widget.insert", json!({"plugin": CARD}));
        assert!(e.contains("path"), "{path}: {e}");
        assert_eq!(crate::plugins_tests::layer_count(&s), 0);
    }
}

#[test]
fn what_api_4_added_survives_save_reload_duplicating_and_export() {
    let mut s = session();
    let (w, _) = insert(&mut s);
    ex(&mut s, "widget.click", json!({"id": w, "target": "start"}));
    ex(&mut s, "widget.menuAction", json!({"id": w, "item": "size", "value": "S"}));
    let bytes = figcraft_format::save_bytes(s.doc().unwrap(), None).unwrap();
    let (back, _) = figcraft_format::load_bytes(&bytes).unwrap();
    let mut t = session();
    t.ds_mut().unwrap().doc = Arc::new(back);
    let l = listed(&mut t, w);
    assert_eq!((l["menu"][2]["value"].clone(), l["inputs"].as_array().unwrap().len(), l["state"].clone()), (json!("S"), 2, json!({"n": 2})));
    assert!(l["timer"].is_null() && next_timer(&t).is_none(), "a reopened file has no timers");
    assert!(matches!(t.execute("widget.fireTimer", &json!({"id": w})), Err(EngineError::Disabled(..))));
    assert_eq!(ex(&mut t, "widget.setText", json!({"id": w, "input": "title", "text": "again"}))["state"], json!({"n": 4}), "and still works");
    point(&mut t, PointerKind::Move, w, "Start");
    assert_eq!(t.tool.overlay.widget_tip.as_deref(), Some("Start"));
    // A duplicate has its own menu values.
    s.execute("select.set", &json!({"ids": [w]})).unwrap();
    ex(&mut s, "edit.duplicate", json!({}));
    let copy = s.selection()[0];
    ex(&mut s, "widget.menuAction", json!({"id": copy, "item": "size", "value": "L"}));
    assert_eq!((listed(&mut s, copy)["menu"][2]["value"].clone(), listed(&mut s, w)["menu"][2]["value"].clone()), (json!("L"), json!("S")));
    // So has a pasted copy, with the texts, the parts and the menu of the one copied, and no timer.
    ex(&mut s, "edit.copy", json!({"ids": [w]}));
    let pasted = crate::params::id_of(&ex(&mut s, "edit.paste", json!({}))["ids"][0]).unwrap();
    let l = listed(&mut s, pasted);
    assert_eq!(
        (l["inputs"].as_array().unwrap().len(), l["targets"].clone(), l["menu"][2]["value"].clone(), l["timer"].clone()),
        (2, json!(["start", "stop"]), json!("S"), Value::Null)
    );
    assert_eq!(ex(&mut s, "widget.setText", json!({"id": pasted, "input": "title", "text": "pasted"}))["state"], json!({"n": 4}));
    assert_eq!(state(&mut s, w), json!({"n": 2}), "the original keeps its own state");
    let svg = crate::cmd::file::export_bytes(s.doc().unwrap(), &[w], figcraft_doc::ExportFormat::Svg, 1.0).unwrap();
    let svg = String::from_utf8(svg).unwrap().to_ascii_lowercase();
    assert!(svg.contains("lineargradient") && svg.contains("dddddd"), "{svg}");
}

#[test]
fn plugins_written_for_api_3_get_what_they_always_got() {
    // The same description under API 3: what API 4 added is ignored where API 3 ignored it.
    let plain = |mut v: Value| {
        let root = &mut v["root"];
        root["padding"] = json!(8);
        root["radius"] = json!(4);
        root["children"].as_array_mut().unwrap().retain(|c| c["type"] != "line" && c["type"] != "vector");
        v
    };
    let mut b = W::default();
    let body = b.render(&plain(look(1, "old")));
    let mut s = Session::new();
    install(&mut s, manifest(3), &b.build(&body));
    let (w, r) = insert(&mut s);
    assert_eq!(
        (r["inputs"].clone(), r["menu"].clone(), r["timer"].clone(), r["targets"].clone()),
        (json!([]), json!([]), Value::Null, json!(["start", "stop"]))
    );
    let root = node(&s, w);
    assert!(root.effects.is_empty() && root.fills.is_empty(), "no shadow, no gradient");
    let (title, start) = (node(&s, part(&s, w, "Title")), node(&s, part(&s, w, "Start")));
    assert!(title.plugin_data.is_empty() && start.plugin_data.len() == 1, "only the target: {:?}", start.plugin_data);
    assert_eq!((title.kind.text().unwrap().max_lines, start.width), (0, 100.0));
    point(&mut s, PointerKind::Move, w, "Start");
    assert!(s.ds().unwrap().text_preview.is_none() && s.tool.overlay.widget_tip.is_none());
    point(&mut s, PointerKind::Move, w, "Title");
    assert_eq!(s.tool.overlay.cursor, Cursor::Default);
    assert!(err(&mut s, "widget.setText", json!({"id": w, "input": "title", "text": "x"})).contains("no text `title`"));
    assert!(err(&mut s, "widget.menuAction", json!({"id": w, "item": "reset"})).contains("no `reset`"));
    // And refused where API 3 refused it: kinds and shapes it didn't have.
    for (root, why) in [
        (json!({"type": "frame", "children": [{"type": "line"}]}), "needs plugin API 4"),
        (json!({"type": "frame", "children": [{"type": "vector", "path": "M0 0 L1 1"}]}), "needs plugin API 4"),
        (json!({"type": "frame", "layout": "vertical", "padding": [1, 2, 3, 4]}), "need plugin API 4"),
    ] {
        let mut b = W::default();
        let body = b.render(&json!({ "root": root }));
        let mut s = Session::new();
        install(&mut s, manifest(3), &b.build(&body));
        assert!(err(&mut s, "widget.insert", json!({"plugin": CARD})).contains(why), "{root}");
    }
}

#[test]
fn junk_layer_data_and_parameters_never_panic() {
    let mut s = session();
    let (w, _) = insert(&mut s);
    // A file (or a peer) may hold anything in a widget's data: it is read as nothing.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut noise = |n: usize| -> String {
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from_u32((seed % 0x2FF) as u32).unwrap_or('\u{0}')
            })
            .collect()
    };
    let junk = [
        "null".to_string(),
        "[]".into(),
        "{".into(),
        r#"{"name": "", "max": 0}"#.into(),
        r#"{"name": "a b", "max": 1e308, "multiline": 3}"#.into(),
        r#"{"name": "title", "max": 18446744073709551615}"#.into(),
        r#"{"fill": 5, "stroke": [1], "opacity": "x"}"#.into(),
        r##"{"fill": "#12", "opacity": 1e308}"##.into(),
        r#"[{"type": "link", "name": "x", "url": "javascript:alert(1)"}]"#.into(),
        r#"[{"type": "dropdown", "name": "x"}]"#.into(),
        format!("[{}]", vec![r#"{"type": "action", "name": "a"}"#; 40].join(",")),
        "x".repeat(200_000),
        noise(300),
        noise(5000),
    ];
    for j in &junk {
        let (title, start) = (part(&s, w, "Title"), part(&s, w, "Start"));
        let doc = s.ds_mut().unwrap().doc_mut();
        for (id, key) in
            [(title, "figcraft.widget.input"), (start, "figcraft.widget.hover"), (start, "figcraft.widget.tooltip"), (w, "figcraft.widget.menu")]
        {
            doc.get_mut(id).unwrap().plugin_data.insert(key.into(), j.clone());
        }
        doc.get_mut(w).unwrap().plugin_data.insert("figcraft.widget.timer".into(), j.clone());
        doc.get_mut(w).unwrap().plugin_data.insert("figcraft.widget.state".into(), j.clone());
        let l = listed(&mut s, w);
        assert_eq!((l["menu"].clone(), l["timer"].clone()), (json!([]), Value::Null), "{}", &j[..j.len().min(60)]);
        assert!(l["inputs"].as_array().unwrap().iter().all(|i| i["maxLength"].as_u64().is_some_and(|m| (1..=1000).contains(&m))));
        // Used as a pointer would: the text first (its junk says it isn't one), then a click that
        // draws the widget anew, then its text for real.
        for name in ["Title", "Start", "Title"] {
            for kind in [PointerKind::Move, PointerKind::Down, PointerKind::Up] {
                point(&mut s, kind, w, name);
            }
            if s.ds().unwrap().text_edit.is_some() {
                crate::textedit::insert(&mut s, "typed").unwrap();
                crate::textedit::key(&mut s, "Enter", false, false, false).unwrap();
            }
        }
        assert!(run_timers(&mut s).is_empty());
    }
    // The new commands with junk parameters: an error each, never a panic.
    let rows = [
        json!({}),
        json!(null),
        json!([1, 2]),
        json!({"id": "x", "input": 5, "text": [1], "item": {"a": 1}, "value": 1e308, "fuel": "NaN"}),
        json!({"id": w, "input": "\u{0}", "text": "\u{0}".repeat(5000), "item": "\u{202e}", "value": {"deep": [[[]]]}, "fuel": -1e308}),
        json!({"id": part(&s, w, "Title"), "input": "title", "text": "x".repeat(100_000), "item": "tint", "value": null, "fuel": 1}),
        json!({"id": 99999, "input": "title", "text": "", "item": "reset"}),
        json!({"id": -1, "target": [1], "input": null, "item": null}),
    ];
    for cmd in ["widget.setText", "widget.menu", "widget.menuAction", "widget.fireTimer", "widget.click", "widget.list", "widget.insert"] {
        for p in &rows {
            if let Err(e) = s.execute(cmd, p) {
                assert!(!matches!(e, EngineError::Internal { .. }), "{cmd} {p}: {e}");
            }
        }
    }
}

/// A session in the room `server` keeps, with the card plugin: (session, its link's script, id).
type Peer = (Session, Arc<Mutex<Script>>, u32);

fn join(server: &mut Server, access: Access) -> Peer {
    let (mut s, script) = crate::live_tests::session();
    install(&mut s, manifest(4), &card(|_| String::new()));
    s.execute("collab.join", &json!({"link": "figcraft://join?host=a:1&file=f&token=t"})).unwrap();
    let ClientMsg::Hello(h) = Client::hello("f", "", "Me", "") else { panic!("no hello") };
    let (id, outs) = server.join_as(h, access).unwrap();
    let welcome = outs.into_iter().find_map(|o| if let Outgoing::To(_, m @ ServerMsg::Welcome { .. }) = o { Some(m) } else { None }).unwrap();
    let mut me = info(Role::Guest, id);
    me.access = access;
    push(&script, vec![Event::Welcome { info: me, peers: vec![] }, Event::Sync(welcome)]);
    s.live_poll();
    (s, script, id)
}

/// Send what each peer did through the server until nobody has anything left to send.
fn pump(server: &mut Server, peers: &mut [Peer]) {
    for _ in 0..8 {
        let mut sent = false;
        for i in 0..peers.len() {
            peers[i].0.live_sync();
            let (msgs, from) = (std::mem::take(&mut peers[i].1.lock().unwrap().msgs), peers[i].2);
            for out in msgs.into_iter().flat_map(|m| server.receive(from, m)) {
                sent = true;
                for (_, script, id) in peers.iter() {
                    let m = match &out {
                        Outgoing::To(to, m) if to == id => m,
                        Outgoing::Others(but, m) if but != id => m,
                        Outgoing::All(m) => m,
                        _ => continue,
                    };
                    if !matches!(m, ServerMsg::Presence { .. }) {
                        push(script, vec![Event::Sync(m.clone())]);
                    }
                }
            }
        }
        peers.iter_mut().for_each(|p| {
            p.0.live_poll();
        });
        if !sent {
            return;
        }
    }
}

#[test]
fn in_a_live_session_updates_sync_and_a_timer_runs_for_whoever_started_it_last() {
    let mut server = Server::new(Session::new().doc().unwrap().clone(), "", figcraft_collab::Limits::default());
    let mut peers = [join(&mut server, Access::Edit), join(&mut server, Access::Edit), join(&mut server, Access::View)];
    let (a, b, viewer) = (0, 1, 2);
    let (w, _) = insert(&mut peers[a].0);
    pump(&mut server, &mut peers);
    assert!(peers.iter().all(|p| p.0.doc().unwrap().node(w).is_some()), "the widget reached everyone");
    // Typed text and a menu choice by one show for the other.
    ex(&mut peers[b].0, "widget.setText", json!({"id": w, "input": "title", "text": "Hi"}));
    pump(&mut server, &mut peers);
    ex(&mut peers[a].0, "widget.menuAction", json!({"id": w, "item": "size", "value": "L"}));
    pump(&mut server, &mut peers);
    for p in &mut peers {
        let l = listed(&mut p.0, w);
        assert_eq!((l["state"].clone(), l["menu"][2]["value"].clone()), (json!({"n": 4}), json!("L")));
    }
    // A starts the timer; B starts it after: only B's fires, and its tick reaches A.
    ex(&mut peers[a].0, "widget.click", json!({"id": w, "target": "start"}));
    pump(&mut server, &mut peers);
    assert!(next_timer(&peers[a].0).is_some() && next_timer(&peers[b].0).is_none(), "the timer is its starter's");
    ex(&mut peers[b].0, "widget.click", json!({"id": w, "target": "start"}));
    pump(&mut server, &mut peers);
    std::thread::sleep(std::time::Duration::from_millis(130));
    assert!(run_timers(&mut peers[a].0).is_empty());
    assert_eq!((state(&mut peers[a].0, w), next_timer(&peers[a].0)), (json!({"n": 2}), None), "A's was replaced: dropped without a callback");
    assert!(run_timers(&mut peers[b].0).is_empty());
    assert_eq!(state(&mut peers[b].0, w), json!({"n": 3}));
    pump(&mut server, &mut peers);
    assert_eq!(state(&mut peers[a].0, w), json!({"n": 3}));
    assert_eq!(server.document().node(w).unwrap().plugin_data.get("figcraft.widget.state").map(String::as_str), Some(r#"{"n":3}"#));
    // The hover look never syncs, and an update arriving under it shows.
    point(&mut peers[a].0, PointerKind::Move, w, "Start");
    assert!(peers[a].0.ds().unwrap().text_preview.is_some());
    ex(&mut peers[b].0, "widget.menuAction", json!({"id": w, "item": "reset"}));
    pump(&mut server, &mut peers);
    assert_eq!(state(&mut peers[a].0, w), json!({"n": 0}));
    let start = part(&peers[b].0, w, "Start");
    assert_eq!(server.document().node(start).unwrap().fills[0].solid_color().unwrap().to_css_hex(), "#0D99FF");
    // Someone who may only view can't use the widget and runs no timers.
    let v = &mut peers[viewer].0;
    assert!(v.execute("widget.click", &json!({"id": w, "target": "start"})).is_err());
    assert!(v.execute("widget.setText", &json!({"id": w, "input": "title", "text": "no"})).is_err());
    assert!(run_timers(v).is_empty() && next_timer(v).is_none());
    assert_eq!(state(v, w), json!({"n": 0}));
}

#[test]
fn a_prototypes_copy_takes_text_menu_items_and_timers_too() {
    let mut s = session();
    let (w, _) = insert(&mut s);
    let editor = s.doc().unwrap().clone();
    let undo = undo_len(&s);
    let (copy, out) = run_on(&s, editor.clone(), "widget.setText", &json!({"id": w, "input": "title", "text": "Hello there"})).unwrap();
    assert_eq!((out["state"].clone(), out["output"]["widget"]["event"]["text"].clone()), (json!({"n": 44}), json!("Hello ther")));
    let (copy, out) = run_on(&s, copy, "widget.menuAction", &json!({"id": w, "item": "reset"})).unwrap();
    assert_eq!(out["state"], json!({"n": 0}));
    // A callback that asks for a timer says so; the player keeps the clock and fires it.
    let (copy, out) = run_on(&s, copy, "widget.click", &json!({"id": w, "target": "start"})).unwrap();
    assert!(out["timer"].as_u64().is_some_and(|t| t <= 100) && out["timerChanged"] == json!(true), "{}", out["timer"]);
    // An event that leaves the timer alone says so; one that stops it, too.
    let (copy, out) = run_on(&s, copy, "widget.menuAction", &json!({"id": w, "item": "pinned"})).unwrap();
    assert_eq!((out["timerChanged"].clone(), out["timer"].clone()), (json!(false), Value::Null));
    let (copy, out) = run_on(&s, copy, "widget.fireTimer", &json!({"id": w})).unwrap();
    assert_eq!((out["state"].clone(), out["timer"].is_u64(), out["timerChanged"].clone()), (json!({"n": 3}), true, json!(true)));
    let (copy, out) = run_on(&s, copy, "widget.click", &json!({"id": w, "target": "stop"})).unwrap();
    assert_eq!((out["timerChanged"].clone(), out["timer"].clone()), (json!(true), Value::Null));
    assert_eq!(copy.node(w).unwrap().plugin_data.get("figcraft.widget.state").map(String::as_str), Some(r#"{"n":3}"#));
    // The file, its history and its timers are as they were.
    assert_eq!((state(&mut s, w), undo_len(&s), next_timer(&s)), (json!({"n": 1}), undo, None));
    assert!(run_on(&s, copy.clone(), "node.delete", &json!({"ids": [w]})).is_err(), "only widget events");
    assert!(run_on(&s, copy, "widget.setText", &json!({"id": w, "input": "nope", "text": ""})).is_err());
}

/// A plugin that shows panel `spec` on every call.
fn panel_plugin(spec: &Value) -> Vec<u8> {
    let mut w = W::default();
    w.imports = r#"(import "figcraft" "ui_show" (func $ui_show (param i32 i32) (result i32)))"#.into();
    let (a, l) = w.s(&spec.to_string());
    w.build(&format!(
        "(if (call $ui_show (i32.const {a}) (i32.const {l})) (then (call $set_output (i32.const 200000) (call $reply_read (i32.const 200000) (i32.const 5000))) (return (i32.const 1))))"
    ))
}

#[test]
fn panels_take_sliders_images_progress_sections_and_disabled_controls() {
    let spec = json!({"title": "Dots", "controls": [
        {"type": "slider", "name": "count", "label": "Count", "min": 2, "max": 12, "step": 2, "value": 7},
        {"type": "slider", "name": "plain"},
        {"type": "image", "label": "Preview", "data": PIXEL, "height": 80, "fit": "fit"},
        {"type": "progress", "label": "Half", "value": 0.5},
        {"type": "divider"},
        {"type": "section", "label": "More", "value": false},
        {"type": "text", "name": "name", "error": "Give it a name"},
        {"type": "button", "name": "go", "label": "Go", "disabled": true},
        {"type": "button", "name": "close", "label": "Close"}
    ]});
    let manifest = |api: u32| json!({"id": "org.test.panel", "name": "Panel", "main": "p.wasm", "api": api, "permissions": {"access": "read"}});
    let mut s = Session::new();
    install(&mut s, manifest(4), &panel_plugin(&spec));
    let r = ex(&mut s, "plugin.run", json!({"id": "org.test.panel"}));
    let c = &r["panel"]["controls"];
    assert_eq!(r["panel"]["values"], json!({"count": 8.0, "plain": 0.0, "name": ""}), "a slider's value sits on a step");
    assert_eq!((c[1]["min"].clone(), c[1]["max"].clone()), (json!(0.0), json!(100.0)));
    assert_eq!(
        (c[3]["default"].clone(), c[5]["default"].clone(), c[6]["error"].clone(), c[7]["disabled"].clone()),
        (json!(0.5), json!(false), json!("Give it a name"), json!(true))
    );
    // A slider's event is clamped and stepped; display controls and disabled ones can't be used.
    let r = ex(&mut s, "plugin.event", json!({"id": "org.test.panel", "control": "count", "value": 99}));
    assert_eq!(r["output"]["event"], json!({"control": "count", "value": 12.0}));
    let r = ex(&mut s, "plugin.event", json!({"id": "org.test.panel", "control": "count", "value": 4.9}));
    assert_eq!(r["output"]["event"]["value"], json!(4.0));
    assert!(err(&mut s, "plugin.event", json!({"id": "org.test.panel", "control": "go"})).contains("disabled"));
    assert!(err(&mut s, "plugin.event", json!({"id": "org.test.panel", "control": "count", "value": "x"})).contains("number"));
    ex(&mut s, "plugin.event", json!({"id": "org.test.panel", "control": "close"}));
    // Under API 2 the same panel is refused: it didn't know these controls.
    let mut s = Session::new();
    install(&mut s, manifest(2), &panel_plugin(&spec));
    assert!(err(&mut s, "plugin.run", json!({"id": "org.test.panel"})).contains("needs plugin API 4"));
    // And what API 4 added to old controls is ignored there, as unknown fields always were.
    let old = json!({"controls": [{"type": "number", "name": "n", "min": 0, "max": 10, "step": 5, "value": 3, "disabled": true, "error": "x"}]});
    install(&mut s, manifest(2), &panel_plugin(&old));
    let r = ex(&mut s, "plugin.run", json!({"id": "org.test.panel"}));
    assert_eq!(r["panel"]["controls"][0], json!({"name": "n", "type": "number", "default": 3.0, "min": 0.0, "max": 10.0}));
    // Junk panels are refused whole.
    for (controls, why) in [
        (json!([{"type": "slider", "name": "s", "step": 0}]), "bad step"),
        (json!([{"type": "slider", "name": "s", "min": 5, "max": 1}]), "bad bounds"),
        (json!([{"type": "image", "label": "No data"}]), "`data`"),
        (json!([{"type": "text", "name": "t", "data": "AAAA"}]), "`data`"),
        (json!([{"type": "image", "data": PIXEL, "height": 4000}]), "high"),
        (json!([{"type": "text", "name": "t", "error": "x".repeat(300)}]), "longer than"),
        (json!([{"type": "slider"}]), "parameter name"),
    ] {
        let mut s = Session::new();
        install(&mut s, manifest(4), &panel_plugin(&json!({ "controls": controls })));
        let e = err(&mut s, "plugin.run", json!({"id": "org.test.panel"}));
        assert!(e.contains(why), "{e}");
    }
}
