//! Pasting a system clipboard image through the canvas shortcut.

use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
use lizard_color::PaintKind;
use lizard_doc::NodeKind;
use lizard_engine::Session;

use crate::{LizardApp, Services};

struct ImageClipboard;

impl Services for ImageClipboard {
    fn clipboard_get_image(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        Some((1200, 800, vec![255; 1200 * 800 * 4]))
    }
}

fn paste(events: Vec<Event>) {
    let mut app = LizardApp::new(Session::new(), Box::new(ImageClipboard));
    let ctx = egui::Context::default();
    for time in [0.0, 1.0 / 60.0] {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
            time: Some(time),
            events: if time == 0.0 { Vec::new() } else { events.clone() },
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.ui(ui));
        out.textures_delta.clear();
    }
    let view = &app.session.ds().unwrap().view;
    let center = (view.cx, view.cy);
    let selection = app.session.selection();
    assert_eq!(selection.len(), 1, "the pasted image is selected");
    let doc = app.session.doc().unwrap();
    let image = doc.node(selection[0]).unwrap();
    assert!(matches!(image.kind, NodeKind::Rectangle));
    assert!(matches!(image.fills[0].kind, PaintKind::Image(_)));
    assert_eq!((image.width, image.height), (1200.0, 800.0));
    let bounds = doc.world_bounds(selection[0]).unwrap();
    assert_eq!((bounds.x0, bounds.y0), ((center.0 - 600.0).round(), (center.1 - 400.0).round()));
    assert_eq!((bounds.x1, bounds.y1), (bounds.x0 + 1200.0, bounds.y0 + 800.0));
}

#[test]
fn cmd_v_pastes_an_image_only_clipboard_at_full_size_in_the_view_center() {
    paste(vec![Event::Key {
        key: Key::V,
        physical_key: Some(Key::V),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::MAC_CMD | Modifiers::COMMAND,
    }]);
}

#[test]
fn a_paste_event_with_text_still_uses_the_clipboard_image() {
    paste(vec![Event::Paste("image alt text".into())]);
}

#[test]
fn an_empty_paste_event_uses_the_clipboard_image() {
    paste(vec![Event::Paste(String::new())]);
}
