//! IME composition in a text layer, driven through egui headless with the events macOS sends for
//! Pinyin ("nihao", then a candidate picked): the platform keeps the IME on every frame of the
//! edit (an idle frame too) with the candidate window at the caret, the composition shows without
//! entering the document, keys pressed while composing are the IME's, and the commit lands once.

use egui::{Event, ImeEvent, Key, Modifiers, RawInput, Rect};
use figcraft_doc::{NodeId, NodeKind};
use figcraft_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;

#[test]
fn gpu_backend_receives_the_transient_ime_document() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::{Arc, Mutex};

    struct Capture(Arc<Mutex<Vec<Arc<figcraft_doc::Document>>>>);
    impl crate::canvas::CanvasGpuBackend for Capture {
        fn texture(
            &mut self,
            doc: Arc<figcraft_doc::Document>,
            _page: NodeId,
            _view: &figcraft_render::View,
            _clear: [f32; 3],
            _gesture_active: bool,
            _proof_srgb: bool,
            _ctx: &egui::Context,
        ) -> Option<egui::TextureId> {
            self.0.lock().ok()?.push(doc);
            None
        }
    }

    let mut session = Session::new();
    let created = session.execute("text.create", &json!({"text": "Hi", "x": 40, "y": 40}))?;
    let id = figcraft_engine::params::id_of(&created["id"]).ok_or("text node missing")?;
    let mut ui = Ui::new(session);
    figcraft_engine::textedit::begin(&mut ui.app.session, id, Some(2), false)?;
    let captured = Arc::new(Mutex::new(Vec::new()));
    ui.app.gpu_canvas = Some(Box::new(Capture(Arc::clone(&captured))));
    let (enabled, _) = enabled_disabled();
    let _ = ime(&mut ui, vec![enabled, preedit("ni")]);
    let _ = ime(&mut ui, vec![]);
    let shown = ui.app.preedit_view().ok_or("IME composition view missing")?;
    let rendered = captured.lock().map_err(|_| "GPU capture lock poisoned")?;
    assert!(rendered.iter().any(|doc| Arc::ptr_eq(doc, &shown.doc)), "GPU backend must receive the composed text document");
    Ok(())
}

fn key(key: Key, pressed: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

fn preedit(text: &str) -> Event {
    let n = text.chars().count();
    Event::Ime(ImeEvent::Preedit { text: text.into(), active_range_chars: (n > 0).then_some(n..n) })
}

#[expect(deprecated, reason = "egui still delivers these, and the editor must ignore them")]
fn enabled_disabled() -> (Event, Event) {
    (Event::Ime(ImeEvent::Enabled), Event::Ime(ImeEvent::Disabled))
}

fn text(u: &Ui, id: NodeId) -> String {
    match &u.app.session.doc().unwrap().node(id).unwrap().kind {
        NodeKind::Text(t) => t.characters.clone(),
        _ => panic!("not text"),
    }
}

/// One frame; the IME output it leaves for the platform (`None` turns the IME off).
fn ime(u: &mut Ui, events: Vec<Event>) -> Rect {
    let ime = u.frame_raw(RawInput { events, ..Default::default() }).platform_output.ime.expect("the IME stays on while editing");
    assert_eq!(ime.rect, ime.cursor_rect, "the desktop places candidates by `rect`: the caret");
    ime.cursor_rect
}

fn composing(u: &Ui) -> Option<String> {
    u.app.session.ds().unwrap().text_edit.as_ref().expect("still editing").preedit.as_ref().map(|p| p.text.clone())
}

#[test]
fn pinyin_composes_inline_and_commits_once_with_the_ime_on_every_frame() {
    // The text is measured with a font: without craft-fonts a system one, scanned here rather than
    // by whichever test happens to run first (#333).
    figcraft_text::scan_system_fonts();
    let mut s = Session::new();
    let t = NodeId(s.execute("text.create", &json!({"text": "Hi", "x": 40, "y": 40})).unwrap()["id"].as_u64().unwrap());
    // On a machine without any font nothing lays out and every caret sits at the start.
    let fonts = figcraft_text::layout_node(s.doc().unwrap().node(t).unwrap()).is_some_and(|(l, _)| !l.glyphs.is_empty());
    let mut u = Ui::new(s);
    figcraft_engine::textedit::begin(&mut u.app.session, t, Some(2), false).unwrap();
    // An idle frame (the caret blinking) keeps the IME on, or macOS drops the composition.
    let caret = ime(&mut u, vec![]);
    let ds = u.app.session.ds().unwrap();
    let b = ds.doc.world_bounds(t).unwrap();
    let layer = Rect::from_two_pos(u.app.to_screen(b.x0, b.y0), u.app.to_screen(b.x1, b.y1)).expand(1.0);
    assert!(layer.contains(caret.center()), "candidates open at the caret ({caret:?} in {layer:?})");

    let (enabled, disabled) = enabled_disabled();
    ime(&mut u, vec![enabled, preedit("n")]);
    ime(&mut u, vec![]);
    let mid = ime(&mut u, vec![preedit("ni")]);
    assert!(!fonts || mid.min.x > caret.min.x, "the IME cursor follows the composition");
    // A key the platform leaks while composing (Backspace) belongs to the IME, not the text.
    ime(&mut u, vec![key(Key::Backspace, true), preedit("nih"), key(Key::Backspace, false)]);
    ime(&mut u, vec![preedit("nihao")]);
    assert_eq!(composing(&u).as_deref(), Some("nihao"));
    assert_eq!(text(&u, t), "Hi", "the composition isn't in the document yet");

    // Enter confirms the candidate: no newline, editing goes on.
    let after = ime(&mut u, vec![key(Key::Enter, true), preedit(""), Event::Ime(ImeEvent::Commit("你好".into())), disabled, key(Key::Enter, false)]);
    assert_eq!(text(&u, t), "Hi你好");
    assert_eq!(composing(&u), None);
    let te = u.app.session.ds().unwrap().text_edit.clone().unwrap();
    assert_eq!((te.caret, te.anchor), (4, 4));
    assert!(!fonts || after.min.x > caret.min.x, "the caret moved past the committed text");
    ime(&mut u, vec![]);
    assert_eq!(text(&u, t), "Hi你好", "committed once");

    // Keys are the editor's again once the composition is over.
    ime(&mut u, vec![key(Key::Enter, true), key(Key::Enter, false), Event::Text("!".into())]);
    assert_eq!(text(&u, t), "Hi你好\n!");
}
